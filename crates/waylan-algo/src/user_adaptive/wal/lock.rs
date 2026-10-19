//! WAL 跨进程排他与共享文件锁
//!
//! # 设计原理
//! - **实现初衷**：在多个桌面会话、设置中心独立进程或并发 CLI 工具同时读写用户自适应词库时，
//!   通过独立的 `.lock` 文件与操作系统内核级 `flock` 协同排他。
//! - **核心优势**：
//!   1. 独立锁文件避免了由于 WAL 归并压缩时原子重命名 (`rename`) 替换数据文件 inode 导致文件锁脱节的缺陷；
//!   2. 支持读写锁语义（共享读与排他写），允许多进程并发重放，同时保证追加写与归并压缩的互斥性；
//!   3. RAII 守卫模式，异常或进程退出时操作系统内核自动释放锁，杜绝死锁。
//! - **代价与局限**：对每个 WAL 文件额外维护一个 0 字节的 `.lock` 文件。

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use rustix::fs::{flock, FlockOperation};

/// 跨进程文件排他与共享锁守卫 (RAII)
pub(crate) struct ProcessFileLock {
    file: File,
}

impl ProcessFileLock {
    /// 获取跨进程排他锁（写锁，互斥阻塞）
    pub(crate) fn acquire_exclusive(path: &Path) -> Option<Self> {
        Self::acquire_internal(path, FlockOperation::LockExclusive)
    }

    /// 获取跨进程共享锁（读锁，允许多读，阻塞排他写）
    pub(crate) fn acquire_shared(path: &Path) -> Option<Self> {
        Self::acquire_internal(path, FlockOperation::LockShared)
    }

    fn acquire_internal(path: &Path, op: FlockOperation) -> Option<Self> {
        let lock_path = Self::lock_path(path);
        if let Some(parent) = lock_path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(&lock_path).ok()?;
        flock(&file, op).ok()?;
        Some(Self { file })
    }

    fn lock_path(path: &Path) -> PathBuf {
        let mut p = path.as_os_str().to_os_string();
        p.push(".lock");
        PathBuf::from(p)
    }
}

impl Drop for ProcessFileLock {
    fn drop(&mut self) {
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_file_lock_lifecycle() {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let wal_path = temp_dir.path().join("test.wal");

        // 测试获取排他锁与自动释放
        {
            let lock1 = ProcessFileLock::acquire_exclusive(&wal_path);
            assert!(lock1.is_some(), "排他锁获取应当成功");
        }

        // 测试获取共享锁
        {
            let lock_shared1 = ProcessFileLock::acquire_shared(&wal_path);
            let lock_shared2 = ProcessFileLock::acquire_shared(&wal_path);
            assert!(lock_shared1.is_some(), "首个共享锁获取应当成功");
            assert!(lock_shared2.is_some(), "并发共享锁获取应当成功");
        }

        let lock_path = ProcessFileLock::lock_path(&wal_path);
        assert!(lock_path.exists(), "专用的 .lock 文件应当被创建");
    }
}
