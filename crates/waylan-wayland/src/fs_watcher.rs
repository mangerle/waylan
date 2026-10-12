//! 文件系统监控与热重载变更高频检测辅助模块
//!
//! 提供基于 Linux `inotify` 的事件驱动文件变更检测，并在不支持时回退为基于元数据时间戳的轻量轮询检测。

use log::{debug, warn};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::inotify::{self, CreateFlags, WatchFlags};
use rustix::io::Errno;
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// 基于 Linux inotify 的词库与配置变更监控器
///
/// # 设计原理
/// - **实现初衷**：消除每 200ms 的主动 stat 轮询，彻底实现由内核驱动的词库与配置热重载感知。
/// - **核心优势**：0% 空闲 CPU 与零系统调用开销，仅在配置文件发生写入、替换或删除时即时唤醒事件循环。
/// - **代价与局限**：为防范文本编辑器写入临时文件并重命名替换原文件，需同时监控目标文件自身及其父目录。
#[derive(Clone, Debug)]
pub struct InotifyWatcher {
    fd: Arc<OwnedFd>,
    watched_paths: Vec<PathBuf>,
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
        let flags = WatchFlags::CLOSE_WRITE
            | WatchFlags::MOVED_TO
            | WatchFlags::CREATE
            | WatchFlags::DELETE
            | WatchFlags::MODIFY;

        let mut watched_dirs = HashSet::new();

        for path in &self.watched_paths {
            if path.exists() {
                if let Err(e) = inotify::add_watch(&self.fd, path, flags) {
                    debug!("[微澜 Inotify] 监听具体文件 {:?} 失败: {}", path, e);
                }
            }
            if let Some(parent) = path.parent() {
                if parent.exists() && watched_dirs.insert(parent.to_path_buf()) {
                    if let Err(e) = inotify::add_watch(&self.fd, parent, flags) {
                        debug!("[微澜 Inotify] 监听父目录 {:?} 失败: {}", parent, e);
                    }
                }
            }
        }
    }

    /// 清空 inotify 事件队列并返回是否有待处理的文件变更事件
    pub fn drain_events(&mut self) -> bool {
        let mut buf = [0u8; 1024];
        let mut has_events = false;

        loop {
            match rustix::io::read(&self.fd, &mut buf) {
                Ok(n) if n > 0 => {
                    has_events = true;
                }
                Err(Errno::WOULDBLOCK) | Ok(_) => break,
                Err(e) => {
                    warn!("[微澜 Inotify] 读取事件缓冲区异常: {}", e);
                    break;
                }
            }
        }

        if has_events {
            // 当检测到文件变动（可能发生新建或移动替换），重新补全 watch 注册
            self.setup_watches();
        }
        has_events
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
}
