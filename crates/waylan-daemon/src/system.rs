//! 系统进程治理、单例防重开、信号处理与偏好设置拉起
//!
//! # 设计原理
//! - **实现初衷**：集中处理微澜进程级别的生命周期事务，包括多实例并发隔离检测、
//!   操作系统信号监听 (SIGINT/SIGTERM)、配置文件监控路径收集以及外部偏好设置界面唤起。
//! - **核心优势**：Linux `/proc` 原生属主 UID 匹配，杜绝多用户误判；环境变量全量继承拉起设置中心。
//! - **代价与局限**：进程遍历依赖 Linux `/proc` 文件系统。

use log::{info, warn};
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use waylan_algo::custom_phrase::CustomPhraseManager;

use crate::config::AppConfig;

/// 注册进程系统退出信号拦截 (SIGINT / SIGTERM)
pub fn register_system_signals(term_now: &Arc<AtomicBool>) {
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        if let Err(e) =
            signal_hook::flag::register_conditional_shutdown(sig, 1, Arc::clone(term_now))
        {
            warn!("[微澜守护进程] 注册信号拦截失败: {}", e);
        }
        if let Err(e) = signal_hook::flag::register(sig, Arc::clone(term_now)) {
            warn!("[微澜守护进程] 注册信号标志置位失败: {}", e);
        }
    }
}

/// 收集被监控热重载的配置文件与词库路径及其初始修改时间
pub fn collect_watched_paths() -> (Vec<PathBuf>, Option<SystemTime>) {
    let mut watched_paths = Vec::with_capacity(3);
    if let Some(c_path) = AppConfig::config_path() {
        watched_paths.push(c_path);
    }
    watched_paths.push(CustomPhraseManager::default_path());
    watched_paths.push(AppConfig::user_aux_dict_path());

    let initial_mtime = watched_paths
        .iter()
        .filter_map(|p| fs::metadata(p).ok().and_then(|m| m.modified().ok()))
        .max();
    (watched_paths, initial_mtime)
}

/// 计算托盘展示形态与独占输入源配置
pub fn determine_tray_and_exclusive(cfg: &AppConfig, use_ibus_backend: bool) -> (bool, bool) {
    let is_gnome = env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false);
    let force_tray = cfg.enable_tray.unwrap_or_else(|| {
        env::var("WAYLAN_ENABLE_TRAY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    });
    let exclusive_source = cfg.exclusive_input_source
        || env::var("WAYLAN_EXCLUSIVE_INPUT_SOURCE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    let should_spawn_tray = force_tray || (!use_ibus_backend && !is_gnome);
    (should_spawn_tray, exclusive_source)
}

/// 唤起独立的微澜偏好设置中心图形界面
pub fn launch_settings_gui() -> Result<(), Box<dyn Error>> {
    info!("[微澜守护进程] 正在拉起独立的偏好设置中心 (waylan-settings)...");
    let current_exe = env::current_exe().ok();
    let sibling_settings = current_exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join("waylan-settings"))
        .filter(|p| p.is_file());

    let mut cmd = if let Some(ref path) = sibling_settings {
        Command::new(path)
    } else {
        Command::new("waylan-settings")
    };

    for var in [
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_CURRENT_DESKTOP",
    ] {
        if let Ok(val) = env::var(var) {
            cmd.env(var, &val);
        }
    }

    cmd.stdin(Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);

    let _child = cmd.spawn()?;
    Ok(())
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
