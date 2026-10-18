//! 系统进程治理、单例防重开、信号处理与偏好设置拉起
//!
//! # 设计原理
//! - **实现初衷**：集中处理微澜进程级别的生命周期事务，包括多实例并发隔离检测、
//!   操作系统信号监听 (SIGINT/SIGTERM)、配置文件监控路径收集以及外部偏好设置界面唤起。
//! - **核心优势**：Linux `/proc` 原生属主 UID 匹配，杜绝多用户误判；环境变量全量继承拉起设置中心。
//! - **代价与局限**：进程遍历依赖 Linux `/proc` 文件系统。

use crate::error::DaemonError;
use log::{info, warn};
use rustix::fs::{flock, FlockOperation};
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{self, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_wayland::LoopWakeup;

use crate::config::AppConfig;

pub mod subprocess;
pub use subprocess::{build_settings_command, SubprocessManager};

/// 进程全局停机协调器
///
/// # 设计原理
/// - **实现初衷**：统一收敛操作系统信号（SIGINT/SIGTERM）、托盘菜单退出/自重启、
///   以及跨线程事件循环唤醒（eventfd / LoopWakeup），杜绝退出标志与唤醒器的分散零乱传递。
/// - **核心优势**：一旦触发停机或自重启，原子标志置位与跨线程事件循环唤醒自动原子联动，
///   保证所有挂起在 poll/epoll 上的事件循环立即脱困退场，实现确定性微秒级优雅停机。
/// - **代价与局限**：跨线程共享依赖 Arc 引用计数。
#[derive(Clone)]
pub struct ShutdownCoordinator {
    exit_requested: Arc<AtomicBool>,
    loop_wakeup: Option<LoopWakeup>,
}

impl ShutdownCoordinator {
    /// 创建新的停机协调器实例
    pub fn new(loop_wakeup: Option<LoopWakeup>) -> Self {
        Self {
            exit_requested: Arc::new(AtomicBool::new(false)),
            loop_wakeup,
        }
    }

    /// 使用现有的原子标志句柄与唤醒器构建（仅供测试）
    #[cfg(test)]
    pub fn from_parts(exit_requested: Arc<AtomicBool>, loop_wakeup: Option<LoopWakeup>) -> Self {
        Self {
            exit_requested,
            loop_wakeup,
        }
    }

    /// 注册进程系统退出信号拦截 (SIGINT / SIGTERM)
    pub fn register_system_signals(&self) {
        let flag = Arc::clone(&self.exit_requested);
        for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
            if let Err(e) =
                signal_hook::flag::register_conditional_shutdown(sig, 1, Arc::clone(&flag))
            {
                warn!("[微澜守护进程] 注册信号拦截失败: {}", e);
            }
            if let Err(e) = signal_hook::flag::register(sig, Arc::clone(&flag)) {
                warn!("[微澜守护进程] 注册信号标志置位失败: {}", e);
            }
        }
    }

    /// 查询当前是否已收到退出或停机请求
    #[inline]
    pub fn is_shutting_down(&self) -> bool {
        self.exit_requested.load(Ordering::Acquire)
    }

    /// 请求优雅停机，并将退出标志置位且立即唤醒主事件循环
    pub fn request_shutdown(&self) {
        self.exit_requested.store(true, Ordering::Release);
        self.wake();
    }

    /// 触发看门狗自重启并在成功后发送停机唤醒
    pub fn trigger_graceful_restart(&self) {
        crate::session_bundle::trigger_graceful_restart_and_exit(
            self.exit_requested.as_ref(),
            self.loop_wakeup.as_ref(),
        );
    }

    /// 触发事件循环唤醒（如果配置了 LoopWakeup）
    pub fn wake(&self) {
        if let Some(ref wakeup) = self.loop_wakeup {
            wakeup.wake();
        }
    }

    /// 获取底层退出标志的 Arc 共享句柄
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exit_requested)
    }

    /// 获取事件循环唤醒器句柄
    pub fn loop_wakeup(&self) -> Option<LoopWakeup> {
        self.loop_wakeup.clone()
    }
}

/// 收集被监控热重载的配置文件与词库路径及其初始状态签名
pub fn collect_watched_paths() -> (Vec<PathBuf>, Option<waylan_core::FileSignature>) {
    let mut watched_paths = Vec::with_capacity(3);
    if let Some(c_path) = AppConfig::config_path() {
        watched_paths.push(c_path);
    }
    watched_paths.push(CustomPhraseManager::default_path());
    watched_paths.push(AppConfig::user_aux_dict_path());

    let initial_sig = waylan_core::compute_initial_signature(&watched_paths);
    (watched_paths, initial_sig)
}

/// 计算托盘展示形态与独占输入源配置
pub fn determine_tray_and_exclusive(cfg: &AppConfig, use_ibus_backend: bool) -> (bool, bool) {
    let is_gnome = env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false);
    let should_spawn_tray = match cfg.desktop.enable_tray {
        waylan_core::TrayDisplayMode::Always => true,
        waylan_core::TrayDisplayMode::Never => false,
        waylan_core::TrayDisplayMode::Auto => {
            let env_force = env::var("WAYLAN_ENABLE_TRAY")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            env_force || (!use_ibus_backend && !is_gnome)
        }
    };
    let exclusive_source = cfg.desktop.exclusive_input_source
        || env::var("WAYLAN_EXCLUSIVE_INPUT_SOURCE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    (should_spawn_tray, exclusive_source)
}

/// 唤起独立的微澜偏好设置中心图形界面
pub fn launch_settings_gui() -> Result<(), DaemonError> {
    info!("[微澜守护进程] 正在拉起独立的偏好设置中心 (waylan-settings)...");
    let mut cmd = build_settings_command();
    cmd.stdin(Stdio::null());
    let _child = cmd.spawn()?;
    Ok(())
}

/// 单实例运行互斥锁守护对象 (RAII)
///
/// # 设计原理
/// - **实现初衷**：消除基于 `/proc` 扫描的 TOCTOU 竞态条件，保证并发启动时仅有一个实例存活。
/// - **核心优势**：基于 Linux 内核级 `flock(2)` 独占锁，进程崩溃或正常退出时内核自动释放锁，避免死锁与脏锁文件残留。
/// - **代价与局限**：必须在守护进程整个运行生命周期内持有该守护对象。
pub struct SingleInstanceLock {
    _file: fs::File,
    path: PathBuf,
}

impl Drop for SingleInstanceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 尝试获取微澜守护进程的全局单例锁
///
/// 若成功获取，返回 `Ok(Some(lock))`；
/// 若锁已被其他运行中的实例独占持有，返回 `Ok(None)`；
/// 若遇到底层 IO 异常，返回 `Err(...)`。
pub fn try_acquire_single_instance_lock() -> std::io::Result<Option<SingleInstanceLock>> {
    let lock_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let lock_path = lock_dir.join(format!(
        "waylan_{}.lock",
        rustix::process::getuid().as_raw()
    ));
    try_acquire_lock_at_path(&lock_path)
}

/// 在指定路径尝试获取独占文件锁（供单元测试与主入口复用）
pub fn try_acquire_lock_at_path(
    lock_path: &std::path::Path,
) -> std::io::Result<Option<SingleInstanceLock>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;

    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {
            let mut file_clone = file.try_clone()?;
            let _ = file_clone.set_len(0);
            let _ = writeln!(file_clone, "{}", process::id());
            Ok(Some(SingleInstanceLock {
                _file: file,
                path: lock_path.to_path_buf(),
            }))
        }
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
        Err(e) => Err(std::io::Error::from(e)),
    }
}

/// 检查当前系统桌面会话中是否已有活跃的微澜输入法守护进程正在运行
pub fn check_another_instance_running() -> bool {
    #[cfg(target_os = "linux")]
    {
        let current_pid = process::id();
        let current_uid = rustix::process::getuid().as_raw();
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Ok(pid) = file_name.parse::<u32>() else {
                    continue;
                };
                if pid == current_pid {
                    continue;
                }
                // 仅匹配同属当前用户的实例，杜绝 Linux 多用户环境下的误判拦截
                if let Ok(meta) = entry.metadata() {
                    if meta.uid() != current_uid {
                        continue;
                    }
                }
                if let Ok(comm) = fs::read_to_string(path.join("comm")) {
                    if comm.trim() == "waylan" {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shutdown_coordinator_lifecycle() {
        let wakeup = LoopWakeup::new().ok();
        let coordinator = ShutdownCoordinator::new(wakeup.clone());

        assert!(!coordinator.is_shutting_down());
        assert!(!coordinator.flag().load(Ordering::Acquire));

        coordinator.request_shutdown();
        assert!(coordinator.is_shutting_down());
        assert!(coordinator.flag().load(Ordering::Acquire));
    }

    #[test]
    fn test_shutdown_coordinator_from_parts() {
        let flag = Arc::new(AtomicBool::new(false));
        let coordinator = ShutdownCoordinator::from_parts(Arc::clone(&flag), None);
        assert!(!coordinator.is_shutting_down());

        coordinator.request_shutdown();
        assert!(coordinator.is_shutting_down());
        assert!(flag.load(Ordering::Acquire));
    }

    #[test]
    fn test_single_instance_lock_mutex() {
        let temp_dir = tempfile::tempdir().expect("创建临时测试目录失败");
        let lock_path = temp_dir.path().join("test_waylan.lock");

        let lock1 = try_acquire_lock_at_path(&lock_path)
            .expect("第一次获取锁失败")
            .expect("第一次应当成功获取锁");

        // 同一路径再次获取应被拦截返回 None
        let lock2 = try_acquire_lock_at_path(&lock_path).expect("探测锁状态失败");
        assert!(lock2.is_none(), "已有锁持有时，不应重复获取锁");

        // 释放第一个锁
        drop(lock1);

        // 释放后再次获取应重新成功
        let lock3 = try_acquire_lock_at_path(&lock_path)
            .expect("重新获取锁失败")
            .expect("释放后应当重新获取锁");
        assert!(lock3.path.exists());
    }
}
