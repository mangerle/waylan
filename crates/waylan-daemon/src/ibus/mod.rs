//! 微澜 (WayLan) IBus 输入法引擎会话管理与总线自动发现模块
//!
//! # 设计原理
//! - **实现初衷**：Ubuntu 默认的 GNOME (Mutter) 桌面环境不向外部暴露 `zwp_input_method_v1` 或
//!   `zwp_input_method_manager_v2` 协议，而是与 `ibus-daemon` 深度集成。本模块通过 D-Bus 协议实现完整的
//!   IBus 引擎后端，使微澜在 GNOME 桌面下既支持被 `ibus-daemon` 作为组件拉起（`--ibus`），也支持在直接运行
//!   `waylan` 时自动发现活跃 IBus 总线并动态注册激活。
//! - **核心优势**：无需 root 权限即可通过 `RegisterComponent` + `SetGlobalEngine` 动态接入当前 GNOME 会话，
//!   同时复用 `inotify` 与 `eventfd` (`LoopWakeup`) 实现零忙轮询的配置热重载与系统托盘联动。
//! - **代价与局限**：依赖宿主机运行中的 `ibus-daemon` 会话实例。

pub mod desktop_integration;
pub mod engine;
pub mod keys;
pub mod types;

use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use log::{debug, info};
use rustix::event::{poll, PollFd, PollFlags};
use waylan_algo::AlgoEngine;
use waylan_wayland::session_common::{
    check_reload_requested_or_files_changed, init_session_inotify_watcher,
};
use waylan_wayland::{InotifyWatcher, LoopWakeup};
use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use crate::config::env_var_non_empty;
use crate::session_bundle::SessionConfigBundle;
use crate::tray::menu::SubprocessManager;
use desktop_integration::{ensure_ibus_and_gnome_integration, format_ibus_engines_xml};
use engine::{
    IbusEngineService, IbusFactoryService, IbusGenericService, WAYLAN_ENGINE_OBJECT_PATH,
    WAYLAN_FACTORY_OBJECT_PATH,
};
use keys::IbusSessionCore;
use types::{build_input_mode_property, build_waylan_ibus_component};

/// IBus 会话启动配置参数包
pub struct IbusSessionOptions {
    /// 已加载词库的拼音算法引擎
    pub engine: AlgoEngine,
    /// 是否由 `ibus-daemon` 通过 `--ibus` 参数作为子进程拉起
    pub is_ibus_child_mode: bool,
    /// 被监控热重载的配置文件与词库路径列表
    pub watched_paths: Vec<PathBuf>,
    /// 初始文件修改时间戳
    pub initial_mtime: Option<SystemTime>,
    /// 与系统托盘共享的中英文模式标志
    pub shared_is_chinese: Option<Arc<AtomicBool>>,
    /// 与系统托盘共享的 CapsLock 大写锁定标志
    pub shared_is_caps_locked: Option<Arc<AtomicBool>>,
    /// 唤醒系统托盘刷新图标的通知通道
    pub mode_notify_tx: Option<SyncSender<()>>,
    /// 与系统托盘共享的重载请求标志
    pub reload_requested: Option<Arc<AtomicBool>>,
    /// 与系统托盘共享的简繁模式标志
    pub shared_traditional_chinese: Option<Arc<AtomicBool>>,
    /// 与系统托盘共享的全角/半角标点标志
    pub shared_full_width_punct: Option<Arc<AtomicBool>>,
    /// 与系统托盘共享的全角/半角字符标志
    pub shared_full_width_char: Option<Arc<AtomicBool>>,
    /// 全局平滑退出标志
    pub exit_requested: Arc<AtomicBool>,
    /// 跨线程 poll 唤醒句柄
    pub loop_wakeup: Option<LoopWakeup>,
    /// 是否静默原生 IBus 属性上报（在独立托盘开启时置为 true 以实现原生顶栏白字指示器自动隐藏）
    pub suppress_properties: bool,
    /// 是否将微澜设为 GNOME 独占单一输入源
    pub exclusive_input_source: bool,
}

/// 探测当前用户会话中的活跃 IBus D-Bus 总线地址
pub fn discover_ibus_address() -> Option<String> {
    if let Some(addr) = env_var_non_empty("IBUS_ADDRESS") {
        if is_ibus_socket_reachable(&addr) {
            return Some(addr);
        }
    }

    let bus_dir = resolve_ibus_bus_dir()?;
    let entries = fs::read_dir(bus_dir).ok()?;
    let prefer_wayland = env_var_non_empty("WAYLAND_DISPLAY").is_some();

    let mut candidates: Vec<(bool, SystemTime, String)> = Vec::with_capacity(4);
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let is_wayland_file = prefer_wayland && file_name.contains("wayland");
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        if let Some(addr) = parse_valid_bus_file(&path) {
            candidates.push((is_wayland_file, mtime, addr));
        }
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    candidates.into_iter().next().map(|(_, _, addr)| addr)
}

/// 解析 `~/.config/ibus/bus` 目录路径
fn resolve_ibus_bus_dir() -> Option<PathBuf> {
    if let Some(cfg_home) = env_var_non_empty("XDG_CONFIG_HOME") {
        let dir = PathBuf::from(cfg_home).join("ibus").join("bus");
        if dir.is_dir() {
            return Some(dir);
        }
    }
    let home = env_var_non_empty("HOME")?;
    let dir = PathBuf::from(home).join(".config").join("ibus").join("bus");
    dir.is_dir().then_some(dir)
}

/// 解析单个 IBus 总线文件并校验守护进程存活与套接字可达性
fn parse_valid_bus_file(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let mut address = None;
    let mut pid: Option<u32> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if let Some(addr) = trimmed.strip_prefix("IBUS_ADDRESS=") {
            address = Some(addr.trim().to_string());
        } else if let Some(p_str) = trimmed.strip_prefix("IBUS_DAEMON_PID=") {
            pid = p_str.trim().parse::<u32>().ok();
        }
    }

    #[cfg(target_os = "linux")]
    if let Some(daemon_pid) = pid {
        if !Path::new(&format!("/proc/{}", daemon_pid)).exists() {
            return None;
        }
    }

    let addr = address?;
    is_ibus_socket_reachable(&addr).then_some(addr)
}

/// 检查 `unix:path=...` 形式的 D-Bus 地址对应的本地套接字文件是否存在
fn is_ibus_socket_reachable(address: &str) -> bool {
    for part in address.split(',') {
        if let Some(sock_path) = part.strip_prefix("unix:path=") {
            return Path::new(sock_path).exists();
        }
        if part.starts_with("unix:abstract=") || part.starts_with("tcp:") {
            return true;
        }
    }
    false
}

/// 将 `SessionConfigBundle` 应用到 `IbusSessionCore` 中（初始化与热重载共用）
fn apply_bundle_to_ibus_core(bundle: SessionConfigBundle, core: &mut IbusSessionCore) {
    bundle.apply_to_engine_and_input(
        &mut core.engine,
        &mut core.shift_sm,
        &mut core.punct_processor,
        &mut core.page_keys,
    );
}

/// 后台监控循环参数结构体
struct IbusMonitorParams {
    watched_paths: Vec<PathBuf>,
    last_mtime: Option<SystemTime>,
    reload_requested: Option<Arc<AtomicBool>>,
    exit_requested: Arc<AtomicBool>,
    loop_wakeup: Option<LoopWakeup>,
}

/// 启动并运行 IBus 输入法会话事件循环
///
/// # Errors
/// 当无法发现活跃 IBus 地址或 D-Bus 连接建立失败时返回错误。
pub fn run_ibus_session(opts: IbusSessionOptions) -> Result<(), Box<dyn Error>> {
    let ibus_addr = discover_ibus_address()
        .ok_or("未检测到活跃的 IBus 会话总线地址，请确认 ibus-daemon 正在运行")?;
    info!("[微澜 IBus] 成功探测到活跃 IBus 总线地址: {}", ibus_addr);

    let IbusSessionOptions {
        engine,
        is_ibus_child_mode,
        watched_paths,
        initial_mtime,
        shared_is_chinese,
        shared_is_caps_locked,
        shared_traditional_chinese,
        shared_full_width_punct,
        shared_full_width_char,
        mode_notify_tx,
        reload_requested,
        exit_requested,
        loop_wakeup,
        suppress_properties,
        exclusive_input_source,
    } = opts;

    let mut session_core = IbusSessionCore::new(engine);
    session_core.shared_is_chinese = shared_is_chinese;
    session_core.shared_is_caps_locked = shared_is_caps_locked;
    session_core.shared_traditional_chinese = shared_traditional_chinese;
    session_core.shared_full_width_punct = shared_full_width_punct;
    session_core.shared_full_width_char = shared_full_width_char;
    session_core.mode_notify_tx = mode_notify_tx;
    apply_bundle_to_ibus_core(SessionConfigBundle::load_current(), &mut session_core);

    let shared_core = Arc::new(Mutex::new(session_core));
    let engine_srv_obj = IbusEngineService {
        core: Arc::clone(&shared_core),
        reload_requested: reload_requested.clone(),
        loop_wakeup: loop_wakeup.clone(),
        subprocess_manager: Arc::new(SubprocessManager::new(Arc::clone(&exit_requested))),
        settings_running: Arc::new(AtomicBool::new(false)),
        suppress_properties,
    };
    let conn = build_ibus_connection(&ibus_addr, &shared_core, engine_srv_obj)?;

    register_or_activate_ibus_engine(&conn, is_ibus_child_mode, exclusive_input_source)?;

    let mut inotify_watcher: Option<InotifyWatcher> = None;
    init_session_inotify_watcher(&watched_paths, &mut inotify_watcher, "IBus");

    let monitor_params = IbusMonitorParams {
        watched_paths,
        last_mtime: initial_mtime,
        reload_requested,
        exit_requested,
        loop_wakeup,
    };

    info!("[微澜 IBus] 引擎服务已就绪，进入 GNOME / IBus 输入法事件循环。");
    run_ibus_monitor_loop(&conn, &shared_core, &mut inotify_watcher, monitor_params);
    cleanup_ibus_session(&shared_core);
    Ok(())
}

/// 会话退出时平滑落盘保存用户自学习词库
fn cleanup_ibus_session(shared_core: &Arc<Mutex<IbusSessionCore>>) {
    if let Ok(mut guard) = shared_core.lock() {
        let _ = guard.engine.persist_user_dict();
        info!("[微澜 IBus] 已安全完成用户自适应历史词库落盘。");
    }
}

/// 建立到 `ibus-daemon` 的 D-Bus 连接并挂载 `Factory`、`Engine` 与 `Service` 接口
fn build_ibus_connection(
    ibus_addr: &str,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    engine: IbusEngineService,
) -> Result<Connection, Box<dyn Error>> {
    let factory = IbusFactoryService {
        core: Arc::clone(shared_core),
    };
    let factory_srv = IbusGenericService {
        core: Arc::clone(shared_core),
    };
    let engine_srv = IbusGenericService {
        core: Arc::clone(shared_core),
    };

    let conn = Builder::address(ibus_addr)?
        .serve_at(WAYLAN_FACTORY_OBJECT_PATH, factory)?
        .serve_at(WAYLAN_FACTORY_OBJECT_PATH, factory_srv)?
        .serve_at(WAYLAN_ENGINE_OBJECT_PATH, engine)?
        .serve_at(WAYLAN_ENGINE_OBJECT_PATH, engine_srv)?
        .build()?;
    Ok(conn)
}

/// 根据启动模式向 `ibus-daemon` 申请总线名或动态注册并激活微澜组件
fn register_or_activate_ibus_engine(
    conn: &Connection,
    is_ibus_child_mode: bool,
    exclusive: bool,
) -> Result<(), Box<dyn Error>> {
    let exe_path = env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "waylan".to_string());

    if is_ibus_child_mode {
        let _ = conn.request_name("org.freedesktop.IBus.Waylan");
        info!("[微澜 IBus] 已在子进程模式下成功申请总线名称 org.freedesktop.IBus.Waylan");
    } else {
        ensure_ibus_and_gnome_integration(&exe_path, exclusive);

        let exec_cmd = format!("{} --ibus", exe_path);
        let setup_cmd = format!("{} settings", exe_path);
        if let Ok(comp_val) = build_waylan_ibus_component(&exec_cmd, &setup_cmd) {
            let _ = conn.call_method(
                Some("org.freedesktop.IBus"),
                "/org/freedesktop/IBus",
                Some("org.freedesktop.IBus"),
                "RegisterComponent",
                &(Value::from(comp_val),),
            );
            info!("[微澜 IBus] 成功向 ibus-daemon 动态注册微澜输入法组件 (org.freedesktop.IBus.Waylan)");
        }
    }

    let _ = conn.request_name("org.freedesktop.IBus.Waylan");

    if let Err(e) = conn.call_method(
        Some("org.freedesktop.IBus"),
        "/org/freedesktop/IBus",
        Some("org.freedesktop.IBus"),
        "SetGlobalEngine",
        &("waylan",),
    ) {
        debug!(
            "[微澜 IBus] 切换全局引擎至 waylan 提示: {}（系统输入源就绪）",
            e
        );
    } else {
        info!("[微澜 IBus] 已成功将当前 GNOME / IBus 全局输入法引擎激活为: waylan (微澜)");
    }

    Ok(())
}

/// 驱动后台文件热重载与托盘状态同步的阻塞监控循环
fn run_ibus_monitor_loop(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    mut params: IbusMonitorParams,
) {
    loop {
        if params.exit_requested.load(Ordering::Acquire) {
            info!("[微澜 IBus] 收到退出请求信号，正在安全退出 IBus 事件循环。");
            break;
        }

        poll_wakeup_and_inotify(inotify_watcher.as_ref(), params.loop_wakeup.as_ref(), 1000);

        if params.exit_requested.load(Ordering::Acquire) {
            info!("[微澜 IBus] 收到退出请求信号，正在安全退出 IBus 事件循环。");
            break;
        }

        let should_reload = check_reload_requested_or_files_changed(
            params.reload_requested.as_deref(),
            inotify_watcher,
            &params.watched_paths,
            &mut params.last_mtime,
        );
        if should_reload {
            let bundle = SessionConfigBundle::load_current();
            let hide_ext = bundle.exclusive_input_source;
            let current_has_tray = shared_core
                .lock()
                .map(|g| g.shared_is_chinese.is_some())
                .unwrap_or(false);
            let new_cfg = crate::config::AppConfig::load_or_create();
            let is_gnome = env::var("XDG_CURRENT_DESKTOP")
                .map(|d| d.to_uppercase().contains("GNOME"))
                .unwrap_or(false);
            let expected_tray = new_cfg.enable_tray.unwrap_or(!is_gnome);

            if expected_tray != current_has_tray {
                info!(
                    "[微澜 IBus] 检测到指示器形态变更 (当前托盘: {}, 期望托盘: {})，正在触发平滑自重启...",
                    current_has_tray, expected_tray
                );
                crate::session_bundle::trigger_graceful_restart_and_exit(
                    params.exit_requested.as_ref(),
                    params.loop_wakeup.as_ref(),
                );
                break;
            }

            if let Ok(mut guard) = shared_core.lock() {
                apply_bundle_to_ibus_core(bundle, &mut guard);
            }
            desktop_integration::ensure_gnome_input_source_registered(hide_ext);
            desktop_integration::sync_gnome_hide_indicator_extension(hide_ext);
            info!("[微澜 IBus] 已完成用户词库与运行配置热重载。");
        }

        let mode_changed = shared_core
            .lock()
            .ok()
            .and_then(|mut g| g.sync_shared_mode().map(|m| (m, g.is_caps_locked)));
        if let Some((new_mode, is_caps)) = mode_changed {
            if let Ok(prop_val) = build_input_mode_property(new_mode, is_caps) {
                let _ = conn.emit_signal(
                    Option::<&str>::None,
                    WAYLAN_ENGINE_OBJECT_PATH,
                    "org.freedesktop.IBus.Engine",
                    "UpdateProperty",
                    &(Value::from(prop_val),),
                );
                debug!("[微澜 IBus] 已同步托盘模式切换并发射 UpdateProperty 信号");
            }
        }
    }
}

/// 使用 `poll` 监听 `inotify` 与跨线程 `eventfd` 唤醒描述符
fn poll_wakeup_and_inotify(
    inotify_watcher: Option<&InotifyWatcher>,
    loop_wakeup: Option<&LoopWakeup>,
    timeout_ms: i32,
) {
    let inotify_fd = inotify_watcher.map(|w| w.fd());
    let wakeup_fd = loop_wakeup.map(|w| w.fd());

    let mut fds: Vec<PollFd<'_>> = Vec::with_capacity(2);
    let wakeup_idx = if let Some(ref fd) = wakeup_fd {
        let idx = fds.len();
        fds.push(PollFd::new(&**fd, PollFlags::IN | PollFlags::ERR));
        Some(idx)
    } else {
        None
    };

    if let Some(ref fd) = inotify_fd {
        fds.push(PollFd::new(&**fd, PollFlags::IN | PollFlags::ERR));
    }

    if fds.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(timeout_ms.max(50) as u64));
        return;
    }

    if let Ok(n) = poll(&mut fds, timeout_ms) {
        if n > 0 {
            if let Some(idx) = wakeup_idx {
                if fds[idx]
                    .revents()
                    .intersects(PollFlags::IN | PollFlags::ERR)
                {
                    if let Some(wakeup) = loop_wakeup {
                        wakeup.drain();
                    }
                }
            }
        }
    }
}

/// 输出符合 IBus `<engines>` 规范的 XML 引擎列表（响应 `waylan --xml`）
pub fn print_ibus_engines_xml() {
    let exe_path = env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "/usr/bin/waylan".to_string());
    let version = env!("CARGO_PKG_VERSION");
    let xml = format_ibus_engines_xml(&exe_path, version);
    let _ = io::stdout().write_all(xml.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ibus_socket_reachable() {
        assert!(is_ibus_socket_reachable(
            "unix:abstract=/tmp/dbus-test,guid=12345"
        ));
        assert!(!is_ibus_socket_reachable(
            "unix:path=/nonexistent/waylan_ibus_test_socket,guid=12345"
        ));
    }
}
