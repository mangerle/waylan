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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// 基于 Linux inotify 的词库与配置变更监控器
///
/// # 设计原理
/// - **实现初衷**：消除每 200ms 的主动 stat 轮询，彻底实现由内核驱动的词库与配置热重载感知。
/// - **核心优势**：0% 空闲 CPU 与零系统调用开销，仅在配置文件发生写入、替换或删除时即时唤醒事件循环。
/// - **代价与局限**：为防范文本编辑器写入临时文件并重命名替换原文件，需同时监控目标文件自身及其父目录；
///   父目录不监听 `MODIFY` 事件并在读取时按文件名精确过滤，杜绝同目录下 `user_dict.bin` 选词追加写引发误触发。
#[derive(Clone, Debug)]
pub struct InotifyWatcher {
    fd: Arc<OwnedFd>,
    watched_paths: Vec<PathBuf>,
    file_watches: HashSet<i32>,
    dir_watches: HashMap<i32, PathBuf>,
}

impl InotifyWatcher {
    /// 尝试为指定文件路径列表构建 inotify 监控实例
    ///
    /// # Errors
    /// 当调用系统 `inotify_init` 失败时返回底层 I/O 错误。
    pub fn try_new(paths: &[PathBuf]) -> io::Result<Self> {
        let fd = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC)?;
        let mut watcher = Self {
            fd: Arc::new(fd),
            watched_paths: paths.to_vec(),
            file_watches: HashSet::with_capacity(paths.len()),
            dir_watches: HashMap::with_capacity(paths.len()),
        };
        watcher.setup_watches();
        Ok(watcher)
    }

    /// 获取底层 inotify 描述符共享句柄
    pub fn fd(&self) -> Arc<OwnedFd> {
        Arc::clone(&self.fd)
    }

    /// 为所有监控文件及其父目录注册 inotify watch 掩码
    fn setup_watches(&mut self) {
        let file_flags = WatchFlags::CLOSE_WRITE
            | WatchFlags::MODIFY
            | WatchFlags::DELETE_SELF
            | WatchFlags::MOVE_SELF;
        let dir_flags = WatchFlags::CLOSE_WRITE
            | WatchFlags::MOVED_TO
            | WatchFlags::CREATE
            | WatchFlags::DELETE;

        self.file_watches.clear();
        self.dir_watches.clear();
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
                // 针对同目录下的用户自适应词库 user_dict.bin：
                // 仅在外部进程（如设置中心）执行删除清空 (DELETE) 或原子归并替换 (MOVED_TO) 时触发热重载，
                // 忽略输入法引擎自身在选词上屏时对 user_dict.bin 的常规创建与追加写操作。
                if os_name == "user_dict.bin"
                    && event
                        .events()
                        .intersects(ReadFlags::DELETE | ReadFlags::MOVED_TO)
                {
                    return true;
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

/// 检查监控文件列表中是否有任何文件发生修改，并在检测到变更时更新基准时间戳（降级回退模式）
///
/// # 设计原理
/// - **实现初衷**：在无法初始化 inotify 的受限环境下，提供零外部依赖的只读 mtime 轮询兜底方案。
/// - **核心优势**：纯标准库元数据只读比较，不依赖内核特定接口。
pub fn check_watched_paths_changed(
    watched_paths: &[PathBuf],
    last_watched_mtime: &mut Option<SystemTime>,
) -> bool {
    if watched_paths.is_empty() {
        return false;
    }

    let mut latest_mtime: Option<SystemTime> = None;
    for p in watched_paths {
        if let Ok(meta) = std::fs::metadata(p) {
            if let Ok(mtime) = meta.modified() {
                latest_mtime = Some(match latest_mtime {
                    Some(prev) => prev.max(mtime),
                    None => mtime,
                });
            }
        }
    }

    match (*last_watched_mtime, latest_mtime) {
        (Some(old), Some(new)) if new != old => {
            *last_watched_mtime = Some(new);
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
    }
}
