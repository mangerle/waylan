//! 基于 Linux inotify 的文件与目录事件监控工具
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心词库、自定义短语及运行时配置文件提供高效的事件驱动文件变动监听，消除主动轮询造成的 CPU 开销。
//! - **核心优势**：基于内核级 inotify 事件唤醒，同时监控目标文件及其父目录（防御文本编辑器原子重命名替换），支持通过过滤器精确短路无关临时文件。
//! - **代价与局限**：受 Linux inotify 实例配额与描述符限制。

use log::{debug, info, warn};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::inotify::{self, CreateFlags, InotifyEvent, ReadFlags, Reader, WatchFlags};
use rustix::io::Errno;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 针对同目录下关联文件变更的判定规则过滤器
pub type DirEventFilter = Arc<dyn Fn(&Path, &OsStr, ReadFlags) -> Option<bool> + Send + Sync>;

/// 构造附带专用落盘时间戳跟踪器的用户词库过滤回调
pub fn user_dict_filter_with_tracker(tracker: Option<Arc<AtomicU64>>) -> DirEventFilter {
    Arc::new(move |_dir, file_name, flags| {
        if file_name == "user_dict.bin" {
            let is_recent = if let Some(ref t) = tracker {
                let last = t.load(Ordering::Acquire);
                if last != 0 {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    now.saturating_sub(last) <= 2000
                } else {
                    false
                }
            } else {
                false
            };

            if is_recent {
                return Some(false);
            }
            if flags.intersects(ReadFlags::DELETE | ReadFlags::MOVED_TO) {
                return Some(true);
            }
        }
        None
    })
}

/// 构造默认的用户词库过滤回调，解耦硬编码文件名与算法状态依赖
pub fn default_user_dict_filter() -> DirEventFilter {
    user_dict_filter_with_tracker(None)
}

/// 基于 Linux inotify 的词库与配置变更监控器
#[derive(Clone)]
pub struct InotifyWatcher {
    fd: Arc<OwnedFd>,
    watched_paths: Vec<PathBuf>,
    file_watches: HashSet<i32>,
    dir_watches: HashMap<i32, PathBuf>,
    event_filter: Option<DirEventFilter>,
}

impl std::fmt::Debug for InotifyWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InotifyWatcher")
            .field("watched_paths", &self.watched_paths)
            .field("file_watches", &self.file_watches)
            .field("dir_watches", &self.dir_watches)
            .finish()
    }
}

impl InotifyWatcher {
    /// 尝试为指定文件路径列表构建 inotify 监控实例
    pub fn try_new(paths: &[PathBuf]) -> io::Result<Self> {
        Self::try_new_with_filter(paths, None)
    }

    /// 尝试为指定文件路径列表构建带有自定义目录事件过滤器的 inotify 监控实例
    pub fn try_new_with_filter(
        paths: &[PathBuf],
        filter: Option<DirEventFilter>,
    ) -> io::Result<Self> {
        let fd = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC)?;
        let mut watcher = Self {
            fd: Arc::new(fd),
            watched_paths: paths.to_vec(),
            file_watches: HashSet::with_capacity(paths.len()),
            dir_watches: HashMap::with_capacity(paths.len()),
            event_filter: filter,
        };
        watcher.setup_watches();
        Ok(watcher)
    }

    /// 设置自定义目录事件过滤器
    pub fn set_event_filter(&mut self, filter: DirEventFilter) {
        self.event_filter = Some(filter);
    }

    /// 获取底层 inotify 描述符共享句柄
    pub fn fd(&self) -> Arc<OwnedFd> {
        Arc::clone(&self.fd)
    }

    /// 为所有监控文件及其父目录注册 inotify watch 掩码，并主动注销已存在的旧描述符防止内核泄漏
    pub fn setup_watches(&mut self) {
        let file_flags = WatchFlags::CLOSE_WRITE
            | WatchFlags::MODIFY
            | WatchFlags::DELETE_SELF
            | WatchFlags::MOVE_SELF;
        let dir_flags = WatchFlags::CLOSE_WRITE
            | WatchFlags::MOVED_TO
            | WatchFlags::CREATE
            | WatchFlags::DELETE;

        for wd in self.file_watches.drain() {
            let _ = inotify::remove_watch(&self.fd, wd);
        }
        for (wd, _) in self.dir_watches.drain() {
            let _ = inotify::remove_watch(&self.fd, wd);
        }
        let mut watched_dirs = HashSet::with_capacity(self.watched_paths.len());

        for path in &self.watched_paths {
            if path.exists() {
                match inotify::add_watch(&self.fd, path, file_flags) {
                    Ok(wd) => {
                        self.file_watches.insert(wd);
                    }
                    Err(e) => {
                        debug!("[微澜 Inotify] 监听具体文件 {:?} 失败: {}", path, e);
                    }
                }
            }
            if let Some(parent) = path.parent() {
                if parent.exists() && watched_dirs.insert(parent.to_path_buf()) {
                    match inotify::add_watch(&self.fd, parent, dir_flags) {
                        Ok(wd) => {
                            self.dir_watches.insert(wd, parent.to_path_buf());
                        }
                        Err(e) => {
                            debug!("[微澜 Inotify] 监听父目录 {:?} 失败: {}", parent, e);
                        }
                    }
                }
            }
        }
    }

    /// 判断单条 inotify 事件是否对应被监控的目标配置文件或外部词库重置操作
    fn is_relevant_event(&self, event: &InotifyEvent<'_>) -> bool {
        let wd = event.wd();
        if self.file_watches.contains(&wd) {
            return true;
        }
        if let Some(dir_path) = self.dir_watches.get(&wd) {
            if let Some(c_name) = event.file_name() {
                let os_name = OsStr::from_bytes(c_name.to_bytes());
                let full_path = dir_path.join(os_name);
                if self.watched_paths.iter().any(|p| p == &full_path) {
                    return true;
                }
                if let Some(ref filter) = self.event_filter {
                    if let Some(decision) = filter(dir_path, os_name, event.events()) {
                        return decision;
                    }
                }
            }
        }
        false
    }

    /// 清空 inotify 事件队列并返回是否有待处理的目标文件变更事件
    pub fn drain_events(&mut self) -> bool {
        let mut buf = [MaybeUninit::uninit(); 1024];
        let mut reader = Reader::new(&self.fd, &mut buf);
        let mut has_relevant_events = false;

        loop {
            match reader.next() {
                Ok(event) => {
                    if self.is_relevant_event(&event) {
                        has_relevant_events = true;
                    }
                }
                Err(Errno::WOULDBLOCK) => break,
                Err(e) => {
                    warn!("[微澜 Inotify] 读取事件缓冲区异常: {}", e);
                    break;
                }
            }
        }

        if has_relevant_events {
            self.setup_watches();
        }
        has_relevant_events
    }

    /// 获取底层 inotify 文件描述符借用
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsFd for InotifyWatcher {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// 被监控文件集合的状态签名（支持真实单文件时间戳与多文件复合哈希指纹，消除伪时间戳坏味道）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSignature {
    /// 单个文件的物理修改时间戳
    SingleFile(SystemTime),
    /// 多文件组合的状态哈希指纹（结合文件存在性、索引位置与修改时间戳）
    MultiFilesHash(u64),
}

/// 检查监控文件列表中是否有任何文件发生修改或被删除，并在检测到变更时更新基准签名（降级回退模式）
pub fn check_watched_paths_changed(
    watched_paths: &[PathBuf],
    last_signature: &mut Option<FileSignature>,
) -> bool {
    if watched_paths.is_empty() {
        return false;
    }

    let mut any_exists = false;
    let mut single_mtime = None;
    let mut combined_hash: u64 = 0xcbf29ce484222325;

    for (idx, p) in watched_paths.iter().enumerate() {
        if let Ok(meta) = std::fs::metadata(p) {
            if let Ok(mtime) = meta.modified() {
                any_exists = true;
                single_mtime = Some(mtime);
                let nanos = mtime
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                combined_hash ^= (idx as u64).wrapping_mul(0x100000001b3);
                combined_hash ^= nanos;
                combined_hash = combined_hash.wrapping_mul(0x100000001b3);
            }
        } else {
            combined_hash ^= (idx as u64).wrapping_add(0x517cc1b727220a95);
            combined_hash = combined_hash.wrapping_mul(0x100000001b3);
        }
    }

    let current_signature = if !any_exists {
        None
    } else if watched_paths.len() == 1 {
        single_mtime.map(FileSignature::SingleFile)
    } else {
        Some(FileSignature::MultiFilesHash(combined_hash))
    };

    match (*last_signature, current_signature) {
        (Some(old), Some(new)) if new != old => {
            *last_signature = Some(new);
            true
        }
        (Some(_), None) => {
            *last_signature = None;
            true
        }
        (None, Some(new)) => {
            *last_signature = Some(new);
            false
        }
        _ => false,
    }
}

/// 计算监控路径列表的初始状态签名
pub fn compute_initial_signature(watched_paths: &[PathBuf]) -> Option<FileSignature> {
    let mut sig = None;
    check_watched_paths_changed(watched_paths, &mut sig);
    sig
}

/// 尝试初始化 inotify 文件监控器
///
/// 若当前已有监控器或监控路径列表为空，则静默跳过；若初始化失败则记录告警并安全降级至时间戳轮询。
pub fn init_session_inotify_watcher(
    watched_paths: &[PathBuf],
    inotify_watcher: &mut Option<InotifyWatcher>,
    session_label: &str,
) {
    init_session_inotify_watcher_with_filter(watched_paths, inotify_watcher, session_label, None);
}

/// 尝试初始化带有自定义目录事件过滤器的 inotify 文件监控器
pub fn init_session_inotify_watcher_with_filter(
    watched_paths: &[PathBuf],
    inotify_watcher: &mut Option<InotifyWatcher>,
    session_label: &str,
    filter: Option<DirEventFilter>,
) {
    if !watched_paths.is_empty() && inotify_watcher.is_none() {
        match InotifyWatcher::try_new_with_filter(watched_paths, filter) {
            Ok(watcher) => {
                info!(
                    "[微澜 {}] 成功初始化 inotify 文件监控，关闭高频后台 stat 轮询。",
                    session_label
                );
                *inotify_watcher = Some(watcher);
            }
            Err(e) => {
                warn!(
                    "[微澜 {}] 初始化 inotify 失败 ({})，回退至定时轮询模式。",
                    session_label, e
                );
            }
        }
    }
}

/// 检查热重载请求或被监控文件是否发生变化
///
/// 优先排空 inotify 队列事件，回退时比对被监控文件状态签名记录。
pub fn check_reload_requested_or_files_changed(
    reload_requested: Option<&AtomicBool>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    watched_paths: &[PathBuf],
    last_signature: &mut Option<FileSignature>,
) -> bool {
    let mut should_reload = reload_requested
        .map(|flag| flag.swap(false, Ordering::AcqRel))
        .unwrap_or(false);

    let changed = if let Some(ref mut watcher) = inotify_watcher {
        watcher.drain_events()
    } else {
        check_watched_paths_changed(watched_paths, last_signature)
    };
    if changed {
        should_reload = true;
    }
    should_reload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_reload_requested() {
        let flag = AtomicBool::new(true);
        let mut watcher = None;
        let paths = Vec::new();
        let mut sig = None;

        let needed =
            check_reload_requested_or_files_changed(Some(&flag), &mut watcher, &paths, &mut sig);
        assert!(needed);
        assert!(!flag.load(Ordering::Relaxed));
    }
}
