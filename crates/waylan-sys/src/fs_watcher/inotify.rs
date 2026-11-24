//! 基于 Linux inotify 的文件与目录事件监控器核心实现
//!
//! # 设计原理
//! - **实现初衷**：为核心文件与运行时配置提供高效的内核级事件驱动文件变动监听，消除主动轮询造成的 CPU 开销。
//! - **核心优势**：基于内核级 inotify 事件唤醒，同时监控目标文件及其父目录（防御文本编辑器原子重命名替换），
//!   支持通过通用过滤器精确短路无关临时文件。
//! - **代价与局限**：受 Linux inotify 实例配额与描述符限制。

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{debug, warn};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
pub use rustix::fs::inotify::ReadFlags;
use rustix::fs::inotify::{self, CreateFlags, Event as InotifyEvent, Reader, WatchFlags};
use rustix::io::Errno;

/// 针对同目录下关联文件变更的判定规则过滤器
pub type DirEventFilter = Arc<dyn Fn(&Path, &OsStr, ReadFlags) -> Option<bool> + Send + Sync>;

/// 时效防抖过滤器单次判定入参
struct TimedFilterJudgement<'a> {
    /// inotify 事件所涉文件名
    file_name: &'a OsStr,
    /// 需要盯防的目标文件名
    target_filename: &'static str,
    /// 外部写入时间戳原子追踪器（None 表示无追踪器，不做时效抑制）
    tracker: Option<&'a Arc<AtomicU64>>,
    /// 抑制防抖时间窗口（毫秒）
    suppress_window_ms: u64,
    /// inotify 事件标志位
    flags: ReadFlags,
}

/// 判断目标文件的最近一次外部写入是否仍落在抑制窗口内
fn is_within_suppress_window(tracker: Option<&Arc<AtomicU64>>, suppress_window_ms: u64) -> bool {
    let Some(tracker) = tracker else {
        return false;
    };
    let last = tracker.load(Ordering::Acquire);
    if last == 0 {
        return false;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now.saturating_sub(last) <= suppress_window_ms
}

/// 依据文件名匹配、时效抑制窗口与事件标志位判定单次目录事件是否应当触发回调
///
/// # 设计原理
/// - **实现初衷**：判定链包含"名称匹配 -> 自写入抑制 -> 删除/移入放行"三段决策，
///   内联在 `Arc<dyn Fn>` 闭包内会让降级分支难以逐段核对，故提取为具名函数。
/// - **核心优势**：返回 `Option<bool>` 精确表达三态语义——`None` 表示与本过滤器无关，
///   `Some(false)` 表示命中目标文件但处于自身写入抑制窗口，`Some(true)` 表示放行。
/// - **代价与局限**：时间戳比对依赖系统时钟单调性，时钟回拨时抑制窗口会被缩短。
fn evaluate_timed_file_event(judgement: &TimedFilterJudgement<'_>) -> Option<bool> {
    if judgement.file_name != judgement.target_filename {
        return None;
    }
    if is_within_suppress_window(judgement.tracker, judgement.suppress_window_ms) {
        return Some(false);
    }
    if judgement
        .flags
        .intersects(ReadFlags::DELETE | ReadFlags::MOVED_TO)
    {
        return Some(true);
    }
    None
}

/// 构造针对指定文件名的时效防抖过滤器（纯通用机制，不绑定具体业务文件名）
///
/// # 参数
/// - `target_filename`: 目标文件名（如 `"user_dict.bin"` 或 `"config.toml"`）
/// - `tracker`: 可选的外部写入时间戳原子追踪器（记录最后一次落盘毫秒时间戳）
/// - `suppress_window_ms`: 抑制防抖时间窗口（毫秒）
pub fn create_timed_file_event_filter(
    target_filename: &'static str,
    tracker: Option<Arc<AtomicU64>>,
    suppress_window_ms: u64,
) -> DirEventFilter {
    Arc::new(move |_dir, file_name, flags| {
        evaluate_timed_file_event(&TimedFilterJudgement {
            file_name,
            target_filename,
            tracker: tracker.as_ref(),
            suppress_window_ms,
            flags,
        })
    })
}

/// 基于 Linux inotify 的通用文件与目录变更监控器
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

    /// 获取底层文件描述符句柄
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
            if let Some(parent) = path.parent()
                && parent.exists()
                && watched_dirs.insert(parent.to_path_buf())
            {
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

    /// 判断单条 inotify 事件是否对应被监控的目标配置文件或外部词库重置操作
    fn is_relevant_event(&self, event: &InotifyEvent<'_>) -> bool {
        let wd = event.wd();
        if self.file_watches.contains(&wd) {
            return true;
        }
        if let Some(dir_path) = self.dir_watches.get(&wd)
            && let Some(c_name) = event.file_name()
        {
            let os_name = OsStr::from_bytes(c_name.to_bytes());
            let full_path = dir_path.join(os_name);
            if self.watched_paths.iter().any(|p| p == &full_path) {
                return true;
            }
            if let Some(ref filter) = self.event_filter
                && let Some(decision) = filter(dir_path, os_name, event.events())
            {
                return decision;
            }
        }
        false
    }

    /// 清空 inotify 事件队列并返回是否有待处理的目标文件变更事件
    pub fn drain_events(&mut self) -> bool {
        let mut buf = [MaybeUninit::uninit(); 1024];
        let mut reader = Reader::new(&self.fd, &mut buf);
        let mut has_relevant_events = false;
        let mut need_rewatch = false;

        loop {
            match reader.next() {
                Ok(event) => {
                    if self.is_relevant_event(&event) {
                        has_relevant_events = true;
                        let flags = event.events();
                        if flags.intersects(
                            ReadFlags::IGNORED
                                | ReadFlags::DELETE_SELF
                                | ReadFlags::MOVE_SELF
                                | ReadFlags::MOVED_TO
                                | ReadFlags::CREATE,
                        ) {
                            need_rewatch = true;
                        }
                    }
                }
                Err(Errno::WOULDBLOCK) => break,
                Err(e) => {
                    warn!("[微澜 Inotify] 读取事件缓冲区异常: {}", e);
                    break;
                }
            }
        }

        if need_rewatch {
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
