//! 文件系统监控与热重载变更高频检测辅助模块
//!
//! 提供基于 Linux `inotify` 的事件驱动文件变更检测，并在不支持时回退为基于元数据时间戳的轻量轮询检测。

use log::{debug, warn};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::inotify::{self, CreateFlags, InotifyEvent, ReadFlags, Reader, WatchFlags};
use rustix::io::Errno;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
                waylan_algo::user_adaptive::is_recent_internal_compact(2000)
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
///
/// # 设计原理
/// - **实现初衷**：消除每 200ms 的主动 stat 轮询，彻底实现由内核驱动的词库与配置热重载感知。
/// - **核心优势**：0% 空闲 CPU 与零系统调用开销，仅在配置文件发生写入、替换或删除时即时唤醒事件循环。
/// - **代价与局限**：为防范文本编辑器写入临时文件并重命名替换原文件，需同时监控目标文件自身及其父目录；
///   父目录不监听 `MODIFY` 事件并在读取时按文件名精确过滤，杜绝同目录下 `user_dict.bin` 选词追加写引发误触发。
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
    /// 尝试为指定文件路径列表构建 inotify 监控实例（默认附带用户自适应词库热重载规则）
    ///
    /// # 设计原理
    /// - **实现初衷**：为输入法核心词库与配置文件变动提供全自动系统级监控，免除手动编写规则代码。
    /// - **核心优势**：默认内建用户词库合并写防误触过滤，自动忽略 2 秒内系统内部合并引发的文件事件。
    /// - **代价与局限**：内置固定过滤策略，若有高度定制化需求需改用 `try_new_with_filter`。
    ///
    /// # Errors
    /// 当调用系统 `inotify_init` 失败时返回底层 I/O 错误。
    pub fn try_new(paths: &[PathBuf]) -> io::Result<Self> {
        Self::try_new_with_filter(paths, Some(default_user_dict_filter()))
    }

    /// 尝试为指定文件路径列表构建带有自定义目录事件过滤器的 inotify 监控实例
    ///
    /// # 设计原理
    /// - **实现初衷**：支持调用方根据特定业务文件命名与事件类型精准注入过滤回调，实现解耦。
    /// - **核心优势**：直接在 Linux 内核事件读取阶段执行闭包短路判断，避免无效全量重载。
    /// - **代价与局限**：回调闭包需满足 `Send + Sync` 约束且在热路径上执行，不应执行阻塞操作。
    ///
    /// # Errors
    /// 当调用系统 `inotify_init` 失败时返回底层 I/O 错误。
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
    fn setup_watches(&mut self) {
        let file_flags = WatchFlags::CLOSE_WRITE
            | WatchFlags::MODIFY
            | WatchFlags::DELETE_SELF
            | WatchFlags::MOVE_SELF;
        let dir_flags = WatchFlags::CLOSE_WRITE
            | WatchFlags::MOVED_TO
            | WatchFlags::CREATE
            | WatchFlags::DELETE;

        // 主动注销内核中旧的 watch 描述符，防止重复 setup 或文件移动时在内核内核表残留
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
                // 通过注入的过滤器代理目录事件判定，避免底层模块硬编码业务文件名
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
            // 当检测到目标文件变动（可能发生新建或移动替换），重新补全 watch 注册
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

/// 检查监控文件列表中是否有任何文件发生修改或被删除，并在检测到变更时更新基准时间戳（降级回退模式）
///
/// # 设计原理
/// - **实现初衷**：在无法初始化 inotify 的受限环境下，提供零外部依赖的只读 mtime 轮询兜底方案。
/// - **核心优势**：纯标准库元数据只读比较，不依赖内核特定接口；采用复合时间指纹，彻底消除单文件 max(mtime) 导致次新文件修改被掩盖的漏洞，并可靠捕获文件删除。
pub fn check_watched_paths_changed(
    watched_paths: &[PathBuf],
    last_watched_mtime: &mut Option<SystemTime>,
) -> bool {
    if watched_paths.is_empty() {
        return false;
    }

    let mut any_exists = false;
    let mut latest_single_mtime = None;
    let mut combined_hash: u64 = 0xcbf29ce484222325;

    for (idx, p) in watched_paths.iter().enumerate() {
        if let Ok(meta) = std::fs::metadata(p) {
            if let Ok(mtime) = meta.modified() {
                any_exists = true;
                latest_single_mtime = Some(mtime);
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

    let latest_mtime = if !any_exists {
        None
    } else if watched_paths.len() == 1 {
        latest_single_mtime
    } else {
        Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(combined_hash))
    };

    match (*last_watched_mtime, latest_mtime) {
        (Some(old), Some(new)) if new != old => {
            *last_watched_mtime = Some(new);
            true
        }
        (Some(_), None) => {
            *last_watched_mtime = None;
            true
        }
        (None, Some(new)) => {
            *last_watched_mtime = Some(new);
            false
        }
        (None, None) => {
            *last_watched_mtime = Some(SystemTime::UNIX_EPOCH);
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_inotify_watcher_detection() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录应当成功");
        let test_file = temp_dir.path().join("test_config.toml");
        {
            let mut f = File::create(&test_file).expect("创建测试文件");
            f.write_all(b"key = 1\n").expect("写入初始数据");
        }

        let mut watcher = InotifyWatcher::try_new(std::slice::from_ref(&test_file))
            .expect("创建 InotifyWatcher 应当成功");

        // 初始状态下无挂起事件
        assert!(!watcher.drain_events());

        // 修改文件内容并写入
        {
            let mut f = File::create(&test_file).expect("打开测试文件覆盖");
            f.write_all(b"key = 2\n").expect("写入更新数据");
            f.flush().expect("刷盘");
        }

        // inotify 应捕获到修改事件
        assert!(watcher.drain_events());
        // 再次清空应返回 false
        assert!(!watcher.drain_events());
    }

    #[test]
    fn test_inotify_ignores_sibling_wal_append_but_detects_reset_and_replace() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录应当成功");
        let watched_aux = temp_dir.path().join("aux.dict.bin");
        let sibling_user_dict = temp_dir.path().join("user_dict.bin");
        let sibling_tmp = temp_dir.path().join("config.toml.tmp.999");

        let mut watcher = InotifyWatcher::try_new(std::slice::from_ref(&watched_aux))
            .expect("创建 InotifyWatcher 应当成功");
        assert!(!watcher.drain_events());

        // 1. 模拟同目录下选词上屏首次创建并持续追加写入 user_dict.bin，以及写入无关临时文件
        let mut wal_file = File::create(&sibling_user_dict).expect("创建 user_dict.bin");
        wal_file.write_all(b"WAYLAN_USERWAL02").expect("写魔数");
        wal_file.flush().expect("刷盘");
        assert!(
            !watcher.drain_events(),
            "选词首次创建并追加写入 user_dict.bin 绝不能误触发配置热重载"
        );

        wal_file.write_all(b"record_frame_1").expect("追加词频记录");
        wal_file.flush().expect("刷盘");
        drop(wal_file);
        assert!(
            !watcher.drain_events(),
            "user_dict.bin 常规写入或关闭句柄不应误触发配置热重载"
        );

        {
            let mut tmp_f = File::create(&sibling_tmp).expect("创建临时文件");
            tmp_f.write_all(b"temp").expect("写入临时数据");
        }
        std::fs::remove_file(&sibling_tmp).expect("清理临时文件");
        assert!(
            !watcher.drain_events(),
            "同目录下无关临时文件的创建、修改与删除不应误触发配置热重载"
        );

        // 2. 模拟外部设置中心通过临时文件原子 rename 替换 user_dict.bin (MOVED_TO)
        let wal_tmp = temp_dir.path().join("user_dict.wal.tmp");
        File::create(&wal_tmp)
            .expect("创建归并临时文件")
            .write_all(b"WAYLAN_USERWAL02_COMPACTED")
            .expect("写入归并内容");
        std::fs::rename(&wal_tmp, &sibling_user_dict).expect("原子重命名替换 user_dict.bin");
        assert!(
            watcher.drain_events(),
            "外部通过 rename 原子替换 user_dict.bin 应正常触发热重载"
        );

        // 3. 模拟外部设置中心清空删除 user_dict.bin (DELETE)
        std::fs::remove_file(&sibling_user_dict).expect("删除 user_dict.bin");
        assert!(
            watcher.drain_events(),
            "外部删除清空 user_dict.bin 应正常触发热重载"
        );

        // 4. 模拟进程内部调用 mark_internal_compact 归并落盘时，原子重命名不应误触发热重载
        waylan_algo::user_adaptive::mark_internal_compact();
        let internal_tmp = temp_dir.path().join("user_dict.internal.tmp");
        File::create(&internal_tmp)
            .expect("创建内部归并临时文件")
            .write_all(b"WAYLAN_USERWAL02_INTERNAL")
            .expect("写入内部归并数据");
        std::fs::rename(&internal_tmp, &sibling_user_dict).expect("原子重命名替换 user_dict.bin");
        assert!(
            !watcher.drain_events(),
            "内部 compact_wal 原子重命名 user_dict.bin 绝不能误触发热重载"
        );

        // 5. 模拟生产环境中目标 user_dict.bin 已存在时，内部 compact_wal 再次原子覆盖替换（可能产生 DELETE 事件）
        waylan_algo::user_adaptive::mark_internal_compact();
        let internal_tmp2 = temp_dir.path().join("user_dict.internal2.tmp");
        File::create(&internal_tmp2)
            .expect("创建第二次内部归并临时文件")
            .write_all(b"WAYLAN_USERWAL02_INTERNAL_2")
            .expect("写入第二次内部归并数据");
        std::fs::rename(&internal_tmp2, &sibling_user_dict)
            .expect("原子覆盖替换已存在的 user_dict.bin");
        assert!(
            !watcher.drain_events(),
            "内部 compact_wal 覆盖替换已存在的 user_dict.bin 时绝不能误触发热重载"
        );
    }

    #[test]
    fn test_check_watched_paths_changed_multiple_files_and_deletion() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录");
        let file_a = temp_dir.path().join("a.toml");
        let file_b = temp_dir.path().join("b.toml");
        std::fs::write(&file_a, "a = 1").expect("写文件 a");
        std::fs::write(&file_b, "b = 1").expect("写文件 b");

        let paths = vec![file_a.clone(), file_b.clone()];
        let mut last_mtime = None;

        // 首次调用初始化基准时间戳，返回 false
        assert!(!check_watched_paths_changed(&paths, &mut last_mtime));
        assert!(last_mtime.is_some());

        // 修改文件 b（即便文件 a 时间戳更晚，复合指纹也能捕捉到文件 b 的变动）
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&file_b, "b = 2").expect("修改文件 b");
        assert!(check_watched_paths_changed(&paths, &mut last_mtime));

        // 再次检查无变动，返回 false
        assert!(!check_watched_paths_changed(&paths, &mut last_mtime));

        // 删除全部监控文件，算法应可靠感知删除并返回 true
        std::fs::remove_file(&file_a).expect("删除文件 a");
        std::fs::remove_file(&file_b).expect("删除文件 b");
        assert!(check_watched_paths_changed(&paths, &mut last_mtime));
        assert!(last_mtime.is_none());
    }
}
