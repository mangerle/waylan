//! 系统进程治理、单例防重开、信号处理与偏好设置拉起
//!
//! # 设计原理
//! - **实现初衷**：集中处理微澜进程级别的生命周期事务，包括多实例并发隔离检测、
//!   操作系统信号监听 (SIGINT/SIGTERM)、配置文件监控路径收集以及外部偏好设置界面唤起。
//! - **核心优势**：Linux `/proc` 原生属主 UID 匹配，杜绝多用户误判；环境变量全量继承拉起设置中心。
//! - **代价与局限**：进程遍历依赖 Linux `/proc` 文件系统。

use crate::error::DaemonError;
use log::{info, warn};
use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_sys::LoopWakeup;
pub use waylan_sys::{SingleInstanceLock, try_acquire_lock_at_path};

use crate::config::AppConfig;

pub mod subprocess;
pub use subprocess::{SubprocessManager, build_settings_command};

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
    ///
    /// 在原子置位退出标志的同时，通过 async-signal-safe 的 `eventfd` 写入立即唤醒阻塞在
    /// `poll(..., -1)` 上的主事件循环（避免信号被工作线程捕获导致主线程收不到 EINTR 而挂死）。
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
            if let Some(ref wakeup) = self.loop_wakeup {
                let wakeup_clone = wakeup.clone();
                // SAFETY: 闭包的唯一系统调用为 `LoopWakeup::wake_signal_safe`（单次 `write(2)`），
                // 完全满足 POSIX async-signal-safety 约束，无分配、无锁、无日志。
                let res = unsafe {
                    signal_hook::low_level::register(sig, move || {
                        wakeup_clone.wake_signal_safe();
                    })
                };
                if let Err(e) = res {
                    warn!("[微澜守护进程] 注册信号事件循环唤醒钩子失败: {}", e);
                }
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
        trigger_graceful_restart_and_exit(self.exit_requested.as_ref(), self.loop_wakeup.as_ref());
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
pub fn collect_watched_paths() -> (Vec<PathBuf>, Option<waylan_sys::FileSignature>) {
    let mut watched_paths = Vec::with_capacity(4);
    if let Some(c_path) = AppConfig::config_path() {
        watched_paths.push(c_path);
    }
    watched_paths.push(CustomPhraseManager::default_path());
    watched_paths.push(AppConfig::user_aux_dict_path());
    for path in AppConfig::base_dict_search_paths() {
        if path.exists() {
            watched_paths.push(path);
            break;
        }
    }

    let initial_sig = waylan_sys::compute_initial_signature(&watched_paths);
    (watched_paths, initial_sig)
}

/// 计算守护进程是否需要拉起专属彩色托盘
pub fn determine_tray_should_spawn(cfg: &AppConfig, use_ibus_backend: bool) -> bool {
    let is_gnome = waylan_sys::desktop::is_gnome_desktop();
    match cfg.desktop.enable_tray {
        waylan_core::TrayDisplayMode::Always => true,
        waylan_core::TrayDisplayMode::Never => false,
        waylan_core::TrayDisplayMode::Auto => {
            let env_force = env::var("WAYLAN_ENABLE_TRAY")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            env_force || (!use_ibus_backend && !is_gnome)
        }
    }
}

/// 解析生效的独占输入源意图：配置项与运维环境变量强制位取或
///
/// 环境变量通道供无人值守部署与故障排查使用，必须同时作用于托盘形态与 GNOME 扩展决策，
/// 否则会出现"环境变量已置位但扩展状态未对齐"的分裂。
#[must_use]
pub fn effective_exclusive_source(cfg: &AppConfig) -> bool {
    cfg.desktop.exclusive_input_source
        || env::var("WAYLAN_EXCLUSIVE_INPUT_SOURCE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
}

/// 自重启看门狗子进程使用的固定 Shell 脚本模板
///
/// 采用固定脚本并通过位置参数 ($1, $2, "$@") 传参，由内核保证参数原子性，100% 杜绝 Shell 注入。
const RESTART_WATCHDOG_SCRIPT: &str = "trap '' HUP; pid=\"$1\"; exe=\"$2\"; shift 2; \
    for _ in $(seq 1 50); do if ! kill -0 \"$pid\" 2>/dev/null; then break; fi; sleep 0.1; done; \
    if kill -0 \"$pid\" 2>/dev/null; then exit 1; fi; \
    sleep 0.15; exec \"$exe\" \"$@\"";

/// 构造自重启看门狗子进程命令
fn build_restart_watchdog_command(exe: &Path) -> Command {
    let pid = process::id();
    let args = env::args().skip(1).collect::<Vec<_>>();

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(RESTART_WATCHDOG_SCRIPT);
    cmd.arg("waylan-restart-helper");
    cmd.arg(pid.to_string());
    cmd.arg(exe);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);
    cmd
}

/// 派生延迟等待当前进程退出的新实例并向旧实例发出优雅停机信号
pub fn trigger_graceful_restart_and_exit(
    exit_requested: &AtomicBool,
    loop_wakeup: Option<&LoopWakeup>,
) {
    let exe = match env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log::error!(
                "[进程治理 错误] 无法获取当前程序可执行路径，取消自重启: {}",
                e
            );
            return;
        }
    };

    match build_restart_watchdog_command(&exe).spawn() {
        Ok(_) => {
            info!("[进程治理] 已成功派生自重启看门狗子进程，正在向旧实例发送优雅停机信号...");
            exit_requested.store(true, Ordering::Release);
            if let Some(wakeup) = loop_wakeup {
                wakeup.wake();
            }
        }
        Err(e) => {
            log::error!(
                "[进程治理 错误] 派生自重启看门狗子进程失败: {}，取消退出以保持当前服务存活",
                e
            );
        }
    }
}

/// 唤起独立的微澜偏好设置中心图形界面（在独立进程组分离运行，供短生命周期 CLI 启动路径使用）
///
/// # Errors
/// 当拉起设置中心二进制进程失败时返回对应 I/O 错误。
pub fn launch_settings_gui() -> Result<(), DaemonError> {
    info!("[微澜守护进程] 正在拉起独立的偏好设置中心 (waylan-settings)...");
    let mut cmd = build_settings_command();
    cmd.stdin(Stdio::null());
    let _child = cmd.spawn()?;
    Ok(())
}

/// 尝试获取微澜守护进程的全局单例锁（复用 `waylan-sys` 的 `0600` 权限与防符号链接 `flock(2)` 实现）
///
/// 若成功获取，返回 `Ok(Some(lock))`；
/// 若锁已被其他运行中的实例独占持有，返回 `Ok(None)`；
/// 若遇到底层 I/O 异常，返回 `Err(...)`。
///
/// # Errors
/// 当锁文件被恶意替换为符号链接或底层文件系统无法锁定且发生非 `WOULDBLOCK` 异常时返回错误。
pub fn try_acquire_single_instance_lock() -> std::io::Result<Option<SingleInstanceLock>> {
    waylan_sys::try_acquire_user_runtime_lock("waylan")
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
                // 仅匹配同属当前用户的实例，杜绝 Linux 多用户环境下的误判拦截；
                // 若读取 metadata 失败（如进程瞬间退出或权限受限），直接跳过该目录。
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if meta.uid() != current_uid {
                    continue;
                }
                if let Ok(comm) = fs::read_to_string(path.join("comm"))
                    && comm.trim() == "waylan"
                {
                    return true;
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

        if let Some(ref w) = wakeup {
            w.wake_signal_safe();
            let mut buf = [0u8; 8];
            let read_bytes = rustix::io::read(w.as_fd(), &mut buf)
                .expect("信号安全唤醒后 eventfd 必须立即就绪可读");
            assert_eq!(read_bytes, 8);
            assert!(u64::from_ne_bytes(buf) >= 1);
        }
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
        drop(lock3);
        // 锁文件必须保留，删除锁文件会让并发进程分别锁住新旧 inode，破坏单实例语义
        assert!(lock_path.exists(), "锁文件不应在释放锁时被删除");
    }
}
