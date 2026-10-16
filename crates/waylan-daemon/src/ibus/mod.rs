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
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread::{self, Builder as ThreadBuilder, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use log::{debug, info};
use rustix::event::{poll, PollFd, PollFlags};
use waylan_algo::AlgoEngine;
use waylan_core::CandidateIndexStyle;
use waylan_wayland::session_common::{
    check_reload_requested_or_files_changed, init_session_inotify_watcher,
};
use waylan_wayland::{InotifyWatcher, LoopWakeup};
use zbus::blocking::connection::Builder;
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::Value;

use crate::config::env_var_non_empty;
use crate::session_bundle::SessionConfigBundle;
use crate::tray::menu::SubprocessManager;
use desktop_integration::{
    ensure_ibus_and_gnome_integration, format_ibus_component_xml, format_ibus_engines_xml,
};
use engine::{
    IbusEngineService, IbusFactoryService, IbusGenericService, UiSnapshot,
    WAYLAN_ENGINE_OBJECT_PATH, WAYLAN_FACTORY_OBJECT_PATH,
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
    core.candidate_index_style = bundle.candidate_index_style;
    core.candidate_orientation = bundle.candidate_orientation;
    core.traditional_shortcut = bundle.traditional_shortcut;
    core.full_width_char_shortcut = bundle.full_width_char_shortcut;
    core.full_width_punct_shortcut = bundle.full_width_punct_shortcut;
    core.voice_config = bundle.voice.clone();
    if !core.voice_config.enabled {
        core.voice_controller.unload_models();
    }
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
    exclusive_input_source: bool,
}

/// 基于外部参数装配并初始化 IBus 共享状态、服务实例与循环参数
fn assemble_ibus_services(
    opts: IbusSessionOptions,
) -> (
    Arc<Mutex<IbusSessionCore>>,
    IbusEngineService,
    IbusMonitorParams,
    bool,
    bool,
) {
    let mut session_core = IbusSessionCore::new(opts.engine);
    session_core.shared_is_chinese = opts.shared_is_chinese;
    session_core.shared_is_caps_locked = opts.shared_is_caps_locked;
    session_core.shared_traditional_chinese = opts.shared_traditional_chinese;
    session_core.shared_full_width_punct = opts.shared_full_width_punct;
    session_core.shared_full_width_char = opts.shared_full_width_char;
    session_core.mode_notify_tx = opts.mode_notify_tx;
    apply_bundle_to_ibus_core(SessionConfigBundle::load_current(), &mut session_core);

    let shared_core = Arc::new(Mutex::new(session_core));
    let engine_srv = IbusEngineService {
        core: Arc::clone(&shared_core),
        reload_requested: opts.reload_requested.clone(),
        loop_wakeup: opts.loop_wakeup.clone(),
        subprocess_manager: Arc::new(SubprocessManager::new(Arc::clone(&opts.exit_requested))),
        settings_running: Arc::new(AtomicBool::new(false)),
        suppress_properties: opts.suppress_properties,
        is_password: Arc::new(AtomicBool::new(false)),
    };

    let monitor_params = IbusMonitorParams {
        watched_paths: opts.watched_paths,
        last_mtime: opts.initial_mtime,
        reload_requested: opts.reload_requested,
        exit_requested: opts.exit_requested,
        loop_wakeup: opts.loop_wakeup,
        exclusive_input_source: opts.exclusive_input_source,
    };

    (
        shared_core,
        engine_srv,
        monitor_params,
        opts.is_ibus_child_mode,
        opts.exclusive_input_source,
    )
}

/// 启动并运行 IBus 输入法会话事件循环
///
/// # Errors
/// 当无法发现活跃 IBus 地址或 D-Bus 连接建立失败时返回错误。
pub fn run_ibus_session(opts: IbusSessionOptions) -> Result<(), Box<dyn Error>> {
    let ibus_addr = discover_ibus_address()
        .ok_or("未检测到活跃的 IBus 会话总线地址，请确认 ibus-daemon 正在运行")?;
    info!("[微澜 IBus] 成功探测到活跃 IBus 总线地址: {}", ibus_addr);

    let (shared_core, engine_srv, monitor_params, is_child, exclusive) =
        assemble_ibus_services(opts);
    let conn = build_ibus_connection(&ibus_addr, &shared_core, engine_srv)?;
    register_or_activate_ibus_engine(&conn, is_child, exclusive)?;

    let mut inotify_watcher: Option<InotifyWatcher> = None;
    init_session_inotify_watcher(&monitor_params.watched_paths, &mut inotify_watcher, "IBus");

    info!("[微澜 IBus] 引擎服务已就绪，进入 GNOME / IBus 输入法事件循环。");
    run_ibus_monitor_loop(&conn, &shared_core, &mut inotify_watcher, monitor_params);
    cleanup_ibus_session(&shared_core);
    Ok(())
}

/// 会话退出时平滑落盘保存用户自学习词库
fn cleanup_ibus_session(shared_core: &Arc<Mutex<IbusSessionCore>>) {
    if let Ok(mut guard) = shared_core.lock() {
        if let Err(e) = guard.engine.persist_user_dict() {
            log::error!("[微澜 IBus 错误] 用户自学习词库落盘失败: {}", e);
        } else {
            info!("[微澜 IBus] 已安全完成用户自适应历史词库落盘。");
        }
    }
}

/// 建立到 `ibus-daemon` 的 D-Bus 连接并挂载 `Factory`、`Engine` 与 `Service` 接口
fn build_ibus_connection(
    ibus_addr: &str,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    engine: IbusEngineService,
) -> Result<Connection, Box<dyn Error>> {
    let factory_next_id = Arc::new(AtomicU32::new(2));
    let factory_conn_slot = Arc::new(Mutex::new(None::<Connection>));

    let factory = IbusFactoryService {
        core: Arc::clone(shared_core),
        conn_slot: Arc::clone(&factory_conn_slot),
        engine_template: engine.clone(),
        next_engine_id: factory_next_id,
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

    if let Ok(mut slot) = factory_conn_slot.lock() {
        *slot = Some(conn.clone());
    }

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
        if let Err(e) = conn.request_name("org.freedesktop.IBus.Waylan") {
            log::warn!(
                "[微澜 IBus 警告] 子进程模式下申请总线名称 org.freedesktop.IBus.Waylan 失败: {}",
                e
            );
        } else {
            info!("[微澜 IBus] 已在子进程模式下成功申请总线名称 org.freedesktop.IBus.Waylan");
        }
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

    if let Err(e) = conn.request_name("org.freedesktop.IBus.Waylan") {
        log::warn!(
            "[微澜 IBus 警告] 申请独占总线名称 org.freedesktop.IBus.Waylan 失败: {}",
            e
        );
    }

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

/// 在会话启动阶段探测初始锁屏状态（优先遵循 FreeDesktop 跨桌面规范，回退至 GNOME 专属规范）
fn query_initial_screen_saver_active(conn: &Connection) -> bool {
    // 1. 优先尝试 FreeDesktop 跨桌面统一规范（KDE Plasma、XFCE、Cinnamon 等通用规范）
    if let Ok(reply) = conn.call_method(
        Some("org.freedesktop.ScreenSaver"),
        "/org/freedesktop/ScreenSaver",
        Some("org.freedesktop.ScreenSaver"),
        "GetActive",
        &(),
    ) {
        if let Ok(active) = reply.body().deserialize::<bool>() {
            return active;
        }
    }

    // 2. 回退尝试 GNOME 专属规范（GNOME Mutter / gnome-screensaver）
    if let Ok(reply) = conn.call_method(
        Some("org.gnome.ScreenSaver"),
        "/org/gnome/ScreenSaver",
        Some("org.gnome.ScreenSaver"),
        "GetActive",
        &(),
    ) {
        if let Ok(active) = reply.body().deserialize::<bool>() {
            return active;
        }
    }

    false
}

/// 后台常驻锁屏信号监听器 RAII 句柄，Drop 时自动唤醒并回收工作线程
struct ScreenSaverListenerHandle {
    join_handle: Option<JoinHandle<()>>,
}

impl ScreenSaverListenerHandle {
    fn stop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            // 向总线广播一条 ActiveChanged 信号以即时唤醒阻塞在 iter.next() 的监听线程
            if let Ok(conn) = Connection::session() {
                let _ = conn.emit_signal(
                    None::<&str>,
                    "/org/waylan/Daemon",
                    "org.freedesktop.ScreenSaver",
                    "ActiveChanged",
                    &(false,),
                );
            }
            let _ = handle.join();
        }
    }
}

impl Drop for ScreenSaverListenerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 注册跨桌面锁屏信号匹配规则（涵盖 GNOME、KDE Plasma、XFCE、systemd-logind 等）
fn register_screen_saver_rules(conn: &Connection) {
    let rules = [
        "type='signal',member='ActiveChanged'",
        "type='signal',interface='org.freedesktop.login1.Session',member='Lock'",
        "type='signal',interface='org.freedesktop.login1.Session',member='Unlock'",
    ];
    for rule in rules {
        let _ = conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &rule,
        );
    }
}

/// 解析锁屏信号并原子更新当前状态标志，若状态发生改变则返回 true
fn handle_screen_saver_signal(msg: &zbus::message::Message, screen_locked: &AtomicBool) -> bool {
    let header = msg.header();
    let member = header.member().map(|m| m.as_str());
    match member {
        Some("ActiveChanged") => {
            if let Ok(active) = msg.body().deserialize::<bool>() {
                return screen_locked.swap(active, Ordering::AcqRel) != active;
            }
        }
        Some("Lock") => {
            return !screen_locked.swap(true, Ordering::AcqRel);
        }
        Some("Unlock") => {
            return screen_locked.swap(false, Ordering::AcqRel);
        }
        _ => {}
    }
    false
}

/// 启动后台常驻 D-Bus 锁屏信号被动监听器
///
/// 通过 D-Bus 信号被动订阅（MatchRule）监听桌面锁屏变迁，彻底杜绝在主循环中高频主动轮询同步 RPC。
fn spawn_screen_saver_listener(
    initial_locked: bool,
    loop_wakeup: Option<LoopWakeup>,
    exit_requested: Arc<AtomicBool>,
) -> (Arc<AtomicBool>, ScreenSaverListenerHandle) {
    let screen_locked = Arc::new(AtomicBool::new(initial_locked));
    let screen_locked_clone = Arc::clone(&screen_locked);

    let handle = ThreadBuilder::new()
        .name("waylan-screen-saver-listener".to_string())
        .spawn(move || {
            let Ok(conn) = Connection::session() else {
                return;
            };
            register_screen_saver_rules(&conn);
            let iter = MessageIterator::from(&conn);
            for msg_res in iter {
                if exit_requested.load(Ordering::Acquire) {
                    break;
                }
                let Ok(msg) = msg_res else {
                    break;
                };
                if handle_screen_saver_signal(&msg, &screen_locked_clone) {
                    if let Some(wakeup) = loop_wakeup.as_ref() {
                        wakeup.wake();
                    }
                }
            }
        })
        .ok();

    (
        screen_locked,
        ScreenSaverListenerHandle {
            join_handle: handle,
        },
    )
}

/// 处理锁屏状态变迁与输入法状态同步
fn handle_screen_saver_transition(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    current_screen_locked: bool,
    last_screen_locked: &mut bool,
) {
    if current_screen_locked == *last_screen_locked {
        return;
    }
    if current_screen_locked {
        info!("[微澜 IBus] 检测到系统进入锁屏状态，进入硬件按键直通与密码保护模式。");
        if let Ok(mut guard) = shared_core.lock() {
            guard.is_screen_locked = true;
            guard.reset_context();
        }
    } else {
        info!("[微澜 IBus] 检测到系统屏幕已解锁，正在自动校准并切回微澜输入法。");
        if let Ok(mut guard) = shared_core.lock() {
            guard.is_screen_locked = false;
        }
        ensure_global_engine_active(conn);
        let _ = Command::new("gsettings")
            .args(["set", "org.gnome.desktop.input-sources", "current", "0"])
            .output();
    }
    *last_screen_locked = current_screen_locked;
}

/// 获取当前语音活跃状态与目标 IBus 引擎对象路径
fn query_voice_active_and_path(shared_core: &Arc<Mutex<IbusSessionCore>>) -> (bool, String) {
    if let Ok(guard) = shared_core.lock() {
        let active = guard.is_voice_active();
        let path = guard
            .active_engine_path
            .clone()
            .unwrap_or_else(|| WAYLAN_ENGINE_OBJECT_PATH.to_string());
        (active, path)
    } else {
        (false, WAYLAN_ENGINE_OBJECT_PATH.to_string())
    }
}

/// 发送语音波形候选框与实时预编辑文本更新
fn emit_voice_stream_update(conn: &Connection, target_engine_path: &str, voice_snap: &UiSnapshot) {
    let table_visible = !voice_snap.candidates.is_empty();
    if let Ok(table_val) = types::build_ibus_lookup_table(
        &voice_snap.candidates,
        voice_snap.page_size,
        voice_snap.selected_index,
        voice_snap.orientation,
        voice_snap.index_style,
    ) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdateLookupTable",
            &(Value::from(table_val), table_visible),
        );
    }

    if let Ok(preedit_val) = types::build_ibus_preedit_text(&voice_snap.preedit) {
        let char_len = voice_snap.preedit.chars().count() as u32;
        let visible = !voice_snap.preedit.is_empty();
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdatePreeditText",
            &(
                Value::from(preedit_val),
                char_len,
                visible,
                types::IBUS_ENGINE_PREEDIT_CLEAR,
            ),
        );
    }
}

/// 发射清空候选框与预编辑的信号
fn emit_voice_stream_exit(conn: &Connection, target_engine_path: &str) {
    if let Ok(empty_table) = types::build_ibus_lookup_table(
        &[],
        1,
        0,
        types::IBUS_ORIENTATION_HORIZONTAL,
        CandidateIndexStyle::None,
    ) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdateLookupTable",
            &(Value::from(empty_table), false),
        );
    }
    if let Ok(empty_preedit) = types::build_ibus_preedit_text("") {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdatePreeditText",
            &(
                Value::from(empty_preedit),
                0u32,
                false,
                types::IBUS_ENGINE_PREEDIT_CLEAR,
            ),
        );
    }
}

/// 自动断句文字上屏提交
fn commit_voice_text(conn: &Connection, target_engine_path: &str, commit_text: &str) {
    if commit_text.is_empty() {
        return;
    }
    info!(
        "[微澜 IBus 语音] 自动断句文字上屏 (目标路径: {}): \"{}\"",
        target_engine_path, commit_text
    );
    emit_voice_stream_exit(conn, target_engine_path);
    if let Ok(text_val) = types::build_ibus_text(commit_text) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "CommitText",
            &(Value::from(text_val),),
        );
    }
}

/// 处理单轮语音事件轮询、流式更新与过渡退出
fn handle_voice_stream_cycle(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    target_engine_path: &str,
    is_voice_active: bool,
    last_voice_active: &mut bool,
) {
    if is_voice_active {
        let (voice_snap, voice_commit) = match shared_core.lock() {
            Ok(mut guard) => {
                let commit = guard.poll_voice_events();
                let snap = UiSnapshot::capture(&guard);
                (snap, commit)
            }
            Err(_) => return,
        };
        emit_voice_stream_update(conn, target_engine_path, &voice_snap);
        if let Some(ref commit_text) = voice_commit {
            commit_voice_text(conn, target_engine_path, commit_text);
        }
    } else if *last_voice_active {
        emit_voice_stream_exit(conn, target_engine_path);
    }
    *last_voice_active = is_voice_active;
}

/// 处理词库与配置热重载事件，若触发指示器形态变更自重启则返回 true
fn handle_config_reload_event(
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    params: &mut IbusMonitorParams,
    last_reload_instant: &mut Instant,
) -> bool {
    let now = Instant::now();
    if now.duration_since(*last_reload_instant) < Duration::from_millis(800) {
        debug!("[微澜 IBus] 处于热重载防抖窗口期内，已安全合并重复触发请求。");
        return false;
    }
    *last_reload_instant = now;
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
        return true;
    }

    if let Ok(mut guard) = shared_core.lock() {
        apply_bundle_to_ibus_core(bundle, &mut guard);
    }
    if hide_ext != params.exclusive_input_source {
        params.exclusive_input_source = hide_ext;
        desktop_integration::ensure_gnome_input_source_registered(hide_ext);
        desktop_integration::sync_gnome_hide_indicator_extension(hide_ext);
    }
    info!("[微澜 IBus] 已完成用户词库与运行配置热重载。");
    false
}

/// 同步托盘模式切换并发射 IBus 引擎 UpdateProperty 信号
fn sync_engine_mode_property(conn: &Connection, shared_core: &Arc<Mutex<IbusSessionCore>>) {
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

/// 驱动后台文件热重载与托盘状态同步的阻塞监控循环
fn run_ibus_monitor_loop(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    mut params: IbusMonitorParams,
) {
    let mut startup_healing_rounds: u32 = 25;
    let initial_screen_locked = Connection::session()
        .ok()
        .as_ref()
        .map(query_initial_screen_saver_active)
        .unwrap_or(false);

    if initial_screen_locked {
        if let Ok(mut guard) = shared_core.lock() {
            guard.is_screen_locked = true;
        }
        info!("[微澜 IBus] 启动时检测到系统处于锁屏状态，已进入密码直通保护模式。");
    }

    let (screen_locked_flag, mut listener_handle) = spawn_screen_saver_listener(
        initial_screen_locked,
        params.loop_wakeup.clone(),
        Arc::clone(&params.exit_requested),
    );
    let mut last_screen_locked = initial_screen_locked;
    let mut last_voice_active = false;
    let mut last_reload_instant = Instant::now()
        .checked_sub(Duration::from_secs(10))
        .unwrap_or_else(Instant::now);

    loop {
        if params.exit_requested.load(Ordering::Acquire) {
            info!("[微澜 IBus] 收到退出请求信号，正在安全退出 IBus 事件循环。");
            break;
        }

        if startup_healing_rounds > 0 {
            startup_healing_rounds = startup_healing_rounds.saturating_sub(1);
            ensure_global_engine_active(conn);
        }

        let current_locked = screen_locked_flag.load(Ordering::Acquire);
        handle_screen_saver_transition(conn, shared_core, current_locked, &mut last_screen_locked);

        let (is_voice_active, target_engine_path) = query_voice_active_and_path(shared_core);
        let poll_timeout_ms = if is_voice_active { 60 } else { 1000 };
        poll_wakeup_and_inotify(
            inotify_watcher.as_ref(),
            params.loop_wakeup.as_ref(),
            poll_timeout_ms,
        );

        handle_voice_stream_cycle(
            conn,
            shared_core,
            &target_engine_path,
            is_voice_active,
            &mut last_voice_active,
        );

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
        if should_reload
            && handle_config_reload_event(shared_core, &mut params, &mut last_reload_instant)
        {
            break;
        }

        sync_engine_mode_property(conn, shared_core);
    }

    listener_handle.stop();
}

/// 从 IBus `GetGlobalEngine` 返回的 `IBusEngineDesc` D-Bus 结构体/变体中提取引擎名称
///
/// IBus 中 `IBusEngineDesc` 的 D-Bus 签名通常为 `(sa{sv}ssssssssussssssss)`，
/// 其中索引 0 为类名 `"IBusEngineDesc"`，索引 2 为引擎唯一名称（如 `"waylan"`）。
fn extract_engine_name_from_desc_variant(val: &Value<'_>) -> Option<String> {
    let mut current = val;
    while let Value::Value(inner) = current {
        current = inner;
    }
    if let Value::Structure(st) = current {
        let fields = st.fields();
        if fields.len() >= 3 {
            if let Value::Str(s) = &fields[2] {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// 确保微澜为当前活跃的全局输入法引擎（用于开机启动与桌面服务时差自愈）
fn ensure_global_engine_active(conn: &Connection) {
    let current_engine = conn
        .call_method(
            Some("org.freedesktop.IBus"),
            "/org/freedesktop/IBus",
            Some("org.freedesktop.IBus"),
            "GetGlobalEngine",
            &(),
        )
        .ok()
        .and_then(|reply| {
            let body = reply.body();
            let val = body.deserialize::<Value<'_>>().ok()?;
            extract_engine_name_from_desc_variant(&val)
        });

    let is_waylan = current_engine.as_deref() == Some("waylan");

    if !is_waylan {
        if let Err(e) = conn.call_method(
            Some("org.freedesktop.IBus"),
            "/org/freedesktop/IBus",
            Some("org.freedesktop.IBus"),
            "SetGlobalEngine",
            &("waylan",),
        ) {
            debug!("[微澜 IBus] 自愈激活全局引擎 waylan 重试提示: {}", e);
        } else {
            info!("[微澜 IBus] 开机自愈完成：成功将全局引擎激活为 waylan (微澜)");
            let _ = Command::new("gsettings")
                .args(["set", "org.gnome.desktop.input-sources", "current", "0"])
                .output();
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
        thread::sleep(Duration::from_millis(timeout_ms.max(50) as u64));
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

/// 输出符合 IBus `<component>` 规范的 XML 组件描述文本（响应 `waylan --ibus-component-xml`）
pub fn print_ibus_component_xml() {
    let version = env!("CARGO_PKG_VERSION");
    let xml = format_ibus_component_xml("/usr/bin/waylan", version);
    let _ = io::stdout().write_all(xml.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zbus::zvariant::StructureBuilder;

    #[test]
    fn test_is_ibus_socket_reachable() {
        assert!(is_ibus_socket_reachable(
            "unix:abstract=/tmp/dbus-test,guid=12345"
        ));
        assert!(!is_ibus_socket_reachable(
            "unix:path=/nonexistent/waylan_ibus_test_socket,guid=12345"
        ));
    }

    #[test]
    fn test_extract_engine_name_from_desc_variant() {
        let mut builder = StructureBuilder::new();
        builder.push_value(Value::from("IBusEngineDesc"));
        builder.push_value(Value::from(HashMap::<String, Value>::new()));
        builder.push_value(Value::from("waylan"));
        let st = builder.build();
        let val = Value::Structure(st);
        let extracted = extract_engine_name_from_desc_variant(&val);
        assert_eq!(extracted.as_deref(), Some("waylan"));
    }

    #[test]
    fn test_screen_saver_listener_lifecycle() {
        let wakeup = LoopWakeup::new().ok();
        let exit_flag = Arc::new(AtomicBool::new(false));

        let (locked_flag, mut listener) =
            spawn_screen_saver_listener(false, wakeup.clone(), Arc::clone(&exit_flag));
        assert!(!locked_flag.load(Ordering::Acquire));

        // 稍等 50ms 确保监听线程完成 D-Bus 会话连接与 MatchRule 注册
        thread::sleep(Duration::from_millis(50));
        exit_flag.store(true, Ordering::Release);

        // 停止监听器并安全回收线程
        listener.stop();
    }
}
