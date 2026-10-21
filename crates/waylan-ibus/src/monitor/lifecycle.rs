//! IBus 全局引擎激活与桌面锁屏状态联动
//!
//! # 设计原理
//! - **实现初衷**：在 IBus 宿主启动、屏幕解锁或桌面会话切换时，保障微澜引擎处于全局激活状态，
//!   并在锁屏期间切入密码直通保护。
//! - **核心优势**：双重探活与防抖节流，既保证开机与解锁后 100% 自愈切回微澜，又避免总线 RPC 洪泛。
//! - **代价与局限**：依赖 GNOME gsettings 外部命令与 session D-Bus 接口。

use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};
use parking_lot::Mutex;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::super::keys::IbusSessionCore;
use waylan_sys::process::{safe_gsettings_get, safe_gsettings_set_async};

/// 查询 IBus 当前全局激活的引擎名称
pub(crate) fn query_current_ibus_engine(conn: &Connection) -> Option<String> {
    let reply = conn
        .call_method(
            Some("org.freedesktop.IBus"),
            "/org/freedesktop/IBus",
            Some("org.freedesktop.IBus"),
            "GetGlobalEngine",
            &(),
        )
        .ok()?;
    let body = reply.body();
    let val = body.deserialize::<Value<'_>>().ok()?;
    super::super::session::extract_engine_name_from_desc_variant(&val)
}

/// 确保微澜为当前活跃的全局输入法引擎（用于开机启动与桌面服务时差自愈）
pub(crate) fn ensure_global_engine_active(conn: &Connection) {
    let current_engine = query_current_ibus_engine(conn);
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

/// 检查 GNOME 输入源索引与 IBus 实际全局引擎状态的一致性，并在发生系统异常掉落时精准自愈
///
/// # 设计原理
/// - **实现初衷**：在 Polkit 系统提权弹窗、密码认证对话框关闭后，GNOME Shell 可能由于异步操作取消
///   而将 IBus 引擎遗漏在 xkb:us::eng，引发状态撕裂（GNOME current 仍为 0 但全局引擎丢失）。
/// - **核心优势**：严格校验 GNOME current 必须依然为 0，当用户主动通过 Win+空格 切换至英文源时绝对不干涉。
/// - **代价与局限**：依赖 GSettings 读取超时控制以防阻塞。
pub(crate) fn maybe_heal_engine_desynchronization(
    conn: &Connection,
    is_screen_locked: bool,
) -> bool {
    if is_screen_locked {
        return false;
    }

    // 1. 读取 GNOME 当前系统输入源索引（带 200ms 超时）
    let Some(current_gnome_src) = safe_gsettings_get(
        "org.gnome.desktop.input-sources",
        "current",
        Duration::from_millis(200),
    ) else {
        return false;
    };

    // 仅当 GNOME 系统明确将首位输入源（微澜，索引 0）作为当前活跃源时才校验。
    // 若用户主动按 Win+空格 切换至其他输入源（如索引 1 的英文），微澜绝对不越权干预。
    if !is_gnome_source_on_waylan(&current_gnome_src) {
        return false;
    }

    // 2. 查询 IBus 实际全局生效的引擎
    let current_engine = query_current_ibus_engine(conn);
    let is_actual_waylan = current_engine.as_deref() == Some("waylan");

    if !is_actual_waylan {
        warn!(
            "[微澜 IBus] 检测到输入源状态撕裂（GNOME 系统指示为微澜，但 IBus 实际引擎掉落为 {:?}），正在执行精准自愈...",
            current_engine
        );

        if let Err(e) = conn.call_method(
            Some("org.freedesktop.IBus"),
            "/org/freedesktop/IBus",
            Some("org.freedesktop.IBus"),
            "SetGlobalEngine",
            &("waylan",),
        ) {
            debug!("[微澜 IBus] 状态撕裂自愈向 IBus 调度引擎提示: {}", e);
        }

        // 同步通知 GNOME 重新调度首位输入源
        safe_gsettings_set_async("org.gnome.desktop.input-sources", "current", "0");
        return true;
    }

    false
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

/// 判定 GNOME 系统的输入源当前索引是否指向首位（索引 0，微澜）
///
/// # 设计原理
/// - GNOME 的 GSettings 返回格式可能是 `"uint32 0"` 或 `"0"`。
/// - 用户若切换为英文，则返回 `"uint32 1"` 或 `"1"`。
pub(crate) fn is_gnome_source_on_waylan(gnome_src_output: &str) -> bool {
    gnome_src_output.split_whitespace().last() == Some("0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_gnome_source_on_waylan_various_formats() {
        assert!(is_gnome_source_on_waylan("uint32 0"));
        assert!(is_gnome_source_on_waylan("0"));
        assert!(is_gnome_source_on_waylan("  uint32 0  "));

        assert!(!is_gnome_source_on_waylan("uint32 1"));
        assert!(!is_gnome_source_on_waylan("1"));
        assert!(!is_gnome_source_on_waylan("uint32 2"));
        assert!(!is_gnome_source_on_waylan(""));
    }
}
