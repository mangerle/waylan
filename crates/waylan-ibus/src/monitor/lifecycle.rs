//! IBus 全局引擎激活与桌面锁屏状态联动
//!
//! # 设计原理
//! - **实现初衷**：在 IBus 宿主启动、屏幕解锁或桌面会话切换时，保障微澜引擎处于全局激活状态，
//!   并在锁屏期间切入密码直通保护。
//! - **核心优势**：双重探活与防抖节流，既保证开机与解锁后 100% 自愈切回微澜，又避免总线 RPC 洪泛。
//! - **代价与局限**：依赖 GNOME gsettings 外部命令与 session D-Bus 接口。

use std::sync::Arc;

use log::{debug, info};
use parking_lot::Mutex;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::super::keys::IbusSessionCore;
use waylan_sys::process::safe_gsettings_set_async;

/// 确保微澜为当前活跃的全局输入法引擎（用于开机启动与桌面服务时差自愈）
pub(crate) fn ensure_global_engine_active(conn: &Connection) {
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
            super::super::session::extract_engine_name_from_desc_variant(&val)
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
            safe_gsettings_set_async("org.gnome.desktop.input-sources", "current", "0");
        }
    }
}

/// 处理锁屏状态变迁与输入法状态同步
pub(crate) fn handle_screen_saver_transition(
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
        {
            let mut guard = shared_core.lock();
            guard.is_screen_locked = true;
            guard.reset_context();
        }
    } else {
        info!("[微澜 IBus] 检测到系统屏幕已解锁，正在自动校准并切回微澜输入法。");
        {
            let mut guard = shared_core.lock();
            guard.is_screen_locked = false;
            guard.is_password_context = false;
        }
        ensure_global_engine_active(conn);
        safe_gsettings_set_async("org.gnome.desktop.input-sources", "current", "0");
    }
    *last_screen_locked = current_screen_locked;
}
