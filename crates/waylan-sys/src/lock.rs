//! 跨进程单实例独占文件锁原语（基于内核 `flock(2)` 与 `0600` 权限保护）
//!
//! # 设计原理
//! - **实现初衷**：为输入法主守护进程与偏好设置中心等独立进程提供统一的无竞争单实例互斥能力，
//!   消除 `/proc` 轮询或 PID 文件存在的 TOCTOU 竞态与脏锁残留风险。
//! - **核心优势**：
//!   1. 基于 Linux 内核级 `flock(2)` 非阻塞排他锁，进程异常崩溃退出时内核自动释放锁；
//!   2. 锁文件强制使用 `0600` 权限创建并在打开前拦截符号链接，防止局部链接劫持；
//!   3. `Drop` 自动随文件描述符关闭释放锁，遵循确定性 RAII 生命周期。
//! - **代价与局限**：同一进程内重复获取同一锁文件路径会被底层 `flock` 视为独立打开文件句柄或重入冲突。

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process;

use rustix::fs::{FlockOperation, flock};

/// 单实例运行互斥锁守护对象 (RAII)
pub struct SingleInstanceLock {
    _file: File,
}

/// 尝试在当前用户运行时目录 (`XDG_RUNTIME_DIR`) 获取指定应用名的全局单实例锁
///
/// 若成功获取，返回 `Ok(Some(lock))`；
/// 若锁已被其他活跃进程独占持有，返回 `Ok(None)`；
/// 若底层目录创建或文件锁定发生 I/O 异常，返回 `Err(...)`。
///
/// # Errors
/// 当锁目录不可写、路径被符号链接占用或底层 `flock` 系统调用返回非 `WOULDBLOCK` 错误时返回 I/O 错误。
pub fn try_acquire_user_runtime_lock(app_name: &str) -> io::Result<Option<SingleInstanceLock>> {
    let uid = rustix::process::getuid().as_raw();
    let lock_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join(format!("waylan-runtime-{uid}")));

    if !lock_dir.exists() {
        fs::create_dir_all(&lock_dir)?;
        let _ = fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o700));
    }

    let lock_path = lock_dir.join(format!("{app_name}_{uid}.lock"));
    try_acquire_lock_at_path(&lock_path)
}

/// 在指定路径尝试获取非阻塞排他文件锁（供生产入口与单元测试复用）
///
/// # Errors
/// 当目标路径为符号链接、文件打开失败或 `flock` 返回非 `WOULDBLOCK` 异常时返回 I/O 错误。
pub fn try_acquire_lock_at_path(lock_path: &Path) -> io::Result<Option<SingleInstanceLock>> {
    if let Ok(meta) = fs::symlink_metadata(lock_path)
        && meta.file_type().is_symlink()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("拒绝在符号链接路径上获取单实例锁: {:?}", lock_path),
        ));
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)?;

    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {
            let mut file_clone = file.try_clone()?;
            let _ = file_clone.set_len(0);
            let _ = writeln!(file_clone, "{}", process::id());
            Ok(Some(SingleInstanceLock { _file: file }))
        }
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
        Err(e) => Err(io::Error::from(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn test_try_acquire_lock_exclusivity_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("app.lock");

        let lock1 = try_acquire_lock_at_path(&lock_path)
            .unwrap()
            .expect("首次获取独占锁应成功");
        let lock2 = try_acquire_lock_at_path(&lock_path).unwrap();
        assert!(lock2.is_none(), "并发再次获取同一独占锁应返回 None");

        drop(lock1);
        let lock3 = try_acquire_lock_at_path(&lock_path).unwrap();
        assert!(lock3.is_some(), "释放守卫后应可重新获取独占锁");
    }

    #[test]
    fn test_try_acquire_lock_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.txt");
        fs::write(&target, b"keep").unwrap();
        let link_path = dir.path().join("symlink.lock");
        symlink(&target, &link_path).unwrap();

        let err = try_acquire_lock_at_path(&link_path)
            .err()
            .expect("符号链接锁文件必须被拒绝");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }
}
