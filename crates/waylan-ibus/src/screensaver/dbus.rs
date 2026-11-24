//! D-Bus 锁屏状态查询与信号解析原语
//!
//! # 设计原理
//! - **实现初衷**：将 Session 总线（FreeDesktop / GNOME ScreenSaver）与 System 总线（systemd-logind）
//!   的锁屏探测、MatchRule 注册和消息解析从状态机与线程调度中剥离，保持职责单一。
//! - **核心优势**：仅在后台监听线程和会话初始化阶段执行 D-Bus 交互，彻底隔离按键与焦点热路径。
//! - **代价与局限**：依赖宿主机运行 `systemd-logind` 或实现标准 `ScreenSaver` 接口的桌面会话。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use zbus::connection::{Builder as ConnectionBuilder, Connection};
use zbus::message::Message;
use zbus::{OwnedVariant, Variant};

/// D-Bus 锁屏状态探测单次 RPC 超时阈值（防止目标服务挂起拖死监控线程）
const DBUS_PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// 全局缓存的 Session 与 System 总线探测长连接
static CACHED_SESSION_CONN: Mutex<Option<Connection>> = Mutex::new(None);
static CACHED_SYSTEM_CONN: Mutex<Option<Connection>> = Mutex::new(None);

/// 获取或安全重建可复用的 Session 总线探测长连接
fn get_or_reconnect_session() -> Option<Connection> {
    let mut lock = CACHED_SESSION_CONN.lock();
    if let Some(ref conn) = *lock
        && !conn.is_closed()
    {
        return Some(conn.clone());
    }
    let new_conn = zbus::block_on(async {
        ConnectionBuilder::session()
            .method_timeout(DBUS_PROBE_TIMEOUT)
            .build()
            .await
            .ok()
    });
    *lock = new_conn.clone();
    new_conn
}

/// 获取或安全重建可复用的 System 总线探测长连接
fn get_or_reconnect_system() -> Option<Connection> {
    let mut lock = CACHED_SYSTEM_CONN.lock();
    if let Some(ref conn) = *lock
        && !conn.is_closed()
    {
        return Some(conn.clone());
    }
    let new_conn = zbus::block_on(async {
        ConnectionBuilder::system()
            .method_timeout(DBUS_PROBE_TIMEOUT)
            .build()
            .await
            .ok()
    });
    *lock = new_conn.clone();
    new_conn
}

/// 查询 `systemd-logind` 系统总线中当前会话的 `LockedHint` 属性
pub(crate) fn query_logind_locked_hint() -> bool {
    let Some(conn) = get_or_reconnect_system() else {
        return false;
    };
    if let Ok(reply) = zbus::block_on(async {
        conn.call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1/session/auto",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.login1.Session", "LockedHint"),
        )
        .await
    }) && let Ok(val) = reply.body().deserialize::<OwnedVariant>()
        && let Variant::Bool(locked) = &*val
    {
        return *locked;
    }
    false
}

/// 在会话启动或重连阶段探测初始锁屏状态（优先遵循 FreeDesktop 跨桌面规范，回退至 GNOME 专属规范）
pub(crate) fn query_initial_screen_saver_active(conn: &Connection) -> bool {
    // 1. 优先尝试 FreeDesktop 跨桌面统一规范（KDE Plasma、XFCE、Cinnamon 等通用规范）
    if let Ok(reply) = zbus::block_on(async {
        conn.call_method(
            Some("org.freedesktop.ScreenSaver"),
            "/org/freedesktop/ScreenSaver",
            Some("org.freedesktop.ScreenSaver"),
            "GetActive",
            &(),
        )
        .await
    }) && let Ok(active) = reply.body().deserialize::<bool>()
    {
        return active;
    }

    // 2. 回退尝试 GNOME 专属规范（GNOME Mutter / gnome-screensaver）
    if let Ok(reply) = zbus::block_on(async {
        conn.call_method(
            Some("org.gnome.ScreenSaver"),
            "/org/gnome/ScreenSaver",
            Some("org.gnome.ScreenSaver"),
            "GetActive",
            &(),
        )
        .await
    }) && let Ok(active) = reply.body().deserialize::<bool>()
    {
        return active;
    }

    false
}

/// 快速探测当前系统锁屏状态（优先权威 `systemd-logind` 属性，回退至用户会话连接）
pub(crate) fn query_current_screen_saver_active() -> bool {
    if query_logind_locked_hint() {
        return true;
    }

    get_or_reconnect_session()
        .as_ref()
        .map(query_initial_screen_saver_active)
        .unwrap_or(false)
}

/// 在 Session 总线注册桌面锁屏信号匹配规则（涵盖 GNOME、KDE Plasma、XFCE 等）
pub(crate) fn register_session_screen_saver_rules(conn: &Connection) {
    let rule = "type='signal',member='ActiveChanged'";
    let _ = zbus::block_on(async {
        conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &rule,
        )
        .await
    });
}

/// 在 System 总线注册 `systemd-logind` 会话锁屏与属性变更匹配规则
pub(crate) fn register_system_logind_rules(conn: &Connection) {
    let rules = [
        "type='signal',sender='org.freedesktop.login1',interface='org.freedesktop.login1.Session',member='Lock'",
        "type='signal',sender='org.freedesktop.login1',interface='org.freedesktop.login1.Session',member='Unlock'",
        "type='signal',sender='org.freedesktop.login1',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'",
    ];
    for rule in rules {
        let _ = zbus::block_on(async {
            conn.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "AddMatch",
                &rule,
            )
            .await
        });
    }
}

/// 解析 Session 总线锁屏信号并原子更新状态标志，状态变化时返回 `true`
pub(crate) fn handle_session_screen_saver_signal(
    msg: &Message,
    screen_locked: &AtomicBool,
) -> bool {
    let header = msg.header();
    let member = header.member().map(|m| m.as_str());
    if member == Some("ActiveChanged")
        && let Ok(active) = msg.body().deserialize::<bool>()
    {
        return screen_locked.swap(active, Ordering::AcqRel) != active;
    }
    false
}

/// 解析 System 总线 `systemd-logind` 锁屏信号与 `LockedHint` 属性变更并原子更新状态标志
pub(crate) fn handle_system_logind_signal(msg: &Message, screen_locked: &AtomicBool) -> bool {
    let header = msg.header();
    let member = header.member().map(|m| m.as_str());
    match member {
        Some("Lock") => !screen_locked.swap(true, Ordering::AcqRel),
        Some("Unlock") => screen_locked.swap(false, Ordering::AcqRel),
        Some("PropertiesChanged") => parse_logind_properties_changed(msg, screen_locked),
        _ => false,
    }
}

/// 从 `org.freedesktop.DBus.Properties.PropertiesChanged` 消息中提取 `LockedHint` 变更
fn parse_logind_properties_changed(msg: &Message, screen_locked: &AtomicBool) -> bool {
    let Ok((iface, changed, _invalidated)) =
        msg.body()
            .deserialize::<(String, HashMap<String, OwnedVariant>, Vec<String>)>()
    else {
        return false;
    };
    if iface != "org.freedesktop.login1.Session" {
        return false;
    }
    if let Some(val) = changed.get("LockedHint")
        && let Variant::Bool(locked) = &**val
    {
        return screen_locked.swap(*locked, Ordering::AcqRel) != *locked;
    }
    false
}
