//! IBus D-Bus 会话装配、组件注册与生命周期管理
//!
//! # 设计原理
//! - **实现初衷**：负责建立与 `ibus-daemon` 的 D-Bus 连接，注册并导出 Factory、Engine、Service 接口，
//!   并在会话退出时落盘保存用户自适应历史词库。
//! - **核心优势**：清晰的启动阶段参数装配（`assemble_ibus_services`）与确定的退场清理（`cleanup_ibus_session`）。
//! - **代价与局限**：强依赖宿主机上正常运行的 ibus-daemon 服务。

use std::env;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex};

use log::{debug, info};
use waylan_core::fs_watcher::{init_session_inotify_watcher, InotifyWatcher};
use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::desktop_integration::ensure_ibus_and_gnome_integration;
use super::discovery::discover_ibus_address;
use super::engine::{
    IbusEngineService, IbusFactoryService, IbusGenericService, WAYLAN_ENGINE_OBJECT_PATH,
    WAYLAN_FACTORY_OBJECT_PATH,
};
use super::error::IbusError;
use super::keys::IbusSessionCore;
use super::monitor::{run_ibus_monitor_loop, IbusMonitorParams};
use super::types::build_waylan_ibus_component;
use super::IbusSessionOptions;

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
    let loop_wakeup = opts.coordinator.loop_wakeup().cloned();
    let shared_core = Arc::new(Mutex::new(opts.session_core));
    let engine_srv = IbusEngineService {
        core: Arc::clone(&shared_core),
        reload_requested: opts.reload_requested.clone(),
        loop_wakeup,
        menu_handler: opts.menu_handler,
        settings_running: Arc::new(AtomicBool::new(false)),
        suppress_properties: opts.suppress_properties,
        is_password: Arc::new(AtomicBool::new(false)),
        last_screen_check_ms: Arc::new(AtomicU64::new(0)),
    };

    let monitor_params = IbusMonitorParams {
        watched_paths: opts.watched_paths,
        last_mtime: opts.initial_mtime,
        reload_requested: opts.reload_requested,
        coordinator: opts.coordinator,
        config_reloader: opts.config_reloader,
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
pub fn run_ibus_session(opts: IbusSessionOptions) -> Result<(), IbusError> {
    let ibus_addr = discover_ibus_address().ok_or(IbusError::AddressNotFound)?;
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
        if let Err(e) = guard.runtime.core.engine.persist_user_dict() {
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
) -> Result<Connection, IbusError> {
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
) -> Result<(), IbusError> {
    let exe_path = env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "waylan".to_string());

    if !is_ibus_child_mode {
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
    } else if is_ibus_child_mode {
        info!("[微澜 IBus] 已在子进程模式下成功申请总线名称 org.freedesktop.IBus.Waylan");
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

    if !is_ibus_child_mode {
        ensure_ibus_and_gnome_integration(&exe_path, exclusive);
    }

    Ok(())
}

/// 从 IBus `GetGlobalEngine` 返回的 `IBusEngineDesc` D-Bus 结构体/变体中提取引擎名称
pub(crate) fn extract_engine_name_from_desc_variant(val: &Value<'_>) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zbus::zvariant::StructureBuilder;

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
}
