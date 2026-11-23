//! IBus 全局引擎激活与桌面锁屏状态联动
//!
//! # 设计原理
//! - **实现初衷**：在 IBus 宿主启动、屏幕解锁或桌面会话切换时，保障微澜引擎处于全局激活状态，
//!   并在锁屏期间切入密码直通保护。
//! - **核心优势**：
//!   1. 双重探活与防抖节流，既保证开机与解锁后 100% 自愈切回微澜，又避免总线 RPC 洪泛；
//!   2. 屏幕解锁与引擎对齐解耦：解锁瞬间仅重置内存直通保护，引擎对齐交由事件循环延时防抖触发，
//!      彻底杜绝与 GNOME Shell 自身异步解锁流程撞车引发的“Set global engine failed: 操作被取消”。
//! - **代价与局限**：依赖 GNOME gsettings 外部命令与 session D-Bus 接口。

use std::sync::Arc;

use log::{debug, info};
use parking_lot::Mutex;
use zbus::Variant;
use zbus::connection::Connection;

use super::super::keys::IbusSessionCore;
use waylan_sys::process::safe_gsettings_set_async;

/// 强制将当前 IBus 全局引擎激活为微澜并对齐桌面输入源配置
pub(crate) fn force_activate_global_engine(conn: &Connection) {
    let result: Result<(), zbus::Error> = zbus::block_on(async {
        conn.call_method(
            Some("org.freedesktop.IBus"),
            "/org/freedesktop/IBus",
            Some("org.freedesktop.IBus"),
            "SetGlobalEngine",
            &("waylan",),
        )
        .await?;
        Ok(())
    });

    match result {
        Ok(()) => {
            info!("[微澜 IBus] 已对齐并强制激活全局引擎为 waylan (微澜)");
            safe_gsettings_set_async("org.gnome.desktop.input-sources", "current", "0");
        }
        Err(e) => {
            debug!("[微澜 IBus] 强制对齐全局引擎 waylan 重试提示: {}", e);
        }
    }
}

/// 确保微澜为当前活跃的全局输入法引擎（用于开机启动与桌面服务时差自愈）
pub(crate) fn ensure_global_engine_active(conn: &Connection) {
    let result: Result<bool, zbus::Error> = zbus::block_on(async {
        let reply = conn
            .call_method(
                Some("org.freedesktop.IBus"),
                "/org/freedesktop/IBus",
                Some("org.freedesktop.IBus"),
                "GetGlobalEngine",
                &(),
            )
            .await?;
        let is_waylan = reply
            .body()
            .deserialize::<Variant<'_>>()
            .ok()
            .and_then(|val| super::super::session::extract_engine_name_from_desc_variant(&val))
            .as_deref()
            == Some("waylan");

        if !is_waylan {
            conn.call_method(
                Some("org.freedesktop.IBus"),
                "/org/freedesktop/IBus",
                Some("org.freedesktop.IBus"),
                "SetGlobalEngine",
                &("waylan",),
            )
            .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    });

    match result {
        Ok(true) => {
            info!("[微澜 IBus] 开机自愈完成：成功将全局引擎激活为 waylan (微澜)");
            safe_gsettings_set_async("org.gnome.desktop.input-sources", "current", "0");
        }
        Ok(false) => {}
        Err(e) => {
            debug!("[微澜 IBus] 自愈激活全局引擎 waylan 重试提示: {}", e);
        }
    }
}

/// 处理锁屏状态变迁与输入法状态同步，若检测到屏幕刚刚解锁则返回 true
pub(crate) fn handle_screen_saver_transition(
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    current_screen_locked: bool,
    last_screen_locked: &mut bool,
) -> bool {
    if current_screen_locked == *last_screen_locked {
        return false;
    }
    let mut just_unlocked = false;
    if current_screen_locked {
        info!("[微澜 IBus] 检测到系统进入锁屏状态，进入硬件按键直通与密码保护模式。");
        {
            let mut guard = shared_core.lock();
            guard.is_screen_locked = true;
            guard.runtime.set_protected_context(true);
            guard.reset_context();
        }
    } else {
        info!("[微澜 IBus] 检测到系统屏幕已解锁，重置直通模式并规划延时防抖引擎对齐。");
        {
            let mut guard = shared_core.lock();
            guard.is_screen_locked = false;
            guard.is_password_context = false;
            guard.runtime.set_protected_context(false);
        }
        just_unlocked = true;
    }
    *last_screen_locked = current_screen_locked;
    just_unlocked
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_engine::{AlgoConfig, AlgoEngine};

    #[test]
    fn test_handle_screen_saver_transition_state_flow() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let core = Arc::new(Mutex::new(IbusSessionCore::new(engine)));
        let mut last_locked = false;

        // 1. 无状态变更时保持稳定
        assert!(!handle_screen_saver_transition(
            &core,
            false,
            &mut last_locked
        ));
        assert!(!core.lock().is_screen_locked);

        // 2. 进入锁屏状态
        assert!(!handle_screen_saver_transition(
            &core,
            true,
            &mut last_locked
        ));
        assert!(last_locked);
        assert!(core.lock().is_screen_locked);

        // 3. 屏幕解锁状态变迁：返回 just_unlocked = true 且立即解除直通
        assert!(handle_screen_saver_transition(
            &core,
            false,
            &mut last_locked
        ));
        assert!(!last_locked);
        assert!(!core.lock().is_screen_locked);
        assert!(!core.lock().is_password_context);

        // 4. 重复解锁状态不重复触发
        assert!(!handle_screen_saver_transition(
            &core,
            false,
            &mut last_locked
        ));
    }
}
