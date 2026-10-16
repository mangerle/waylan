//! 桌面锁屏状态被动感知与信号监听
//!
//! # 设计原理
//! - **实现初衷**：在锁屏状态下必须自动直通按键并禁用候选框与中文录入，防止密码泄露与锁屏界面误触发。
//! - **核心优势**：通过 D-Bus 信号被动订阅（MatchRule）监听锁屏变迁，彻底杜绝在主循环中高频主动轮询同步 RPC。
//! - **代价与局限**：监听线程在 Drop 时通过向总线发射本地信号唤醒以安全回收线程。

use crate::IbusShutdownCoordinator;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{Builder as ThreadBuilder, JoinHandle};
use std::time::Duration;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Message;

/// 在会话启动阶段探测初始锁屏状态（优先遵循 FreeDesktop 跨桌面规范，回退至 GNOME 专属规范）
pub(crate) fn query_initial_screen_saver_active(conn: &Connection) -> bool {
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
pub(crate) struct ScreenSaverListenerHandle {
    join_handle: Option<JoinHandle<()>>,
}

impl ScreenSaverListenerHandle {
    pub(crate) fn stop(&mut self) {
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
fn handle_screen_saver_signal(msg: &Message, screen_locked: &AtomicBool) -> bool {
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
pub(crate) fn spawn_screen_saver_listener(
    initial_locked: bool,
    coordinator: IbusShutdownCoordinator,
) -> (Arc<AtomicBool>, ScreenSaverListenerHandle) {
    let screen_locked = Arc::new(AtomicBool::new(initial_locked));
    let screen_locked_clone = Arc::clone(&screen_locked);

    let handle = ThreadBuilder::new()
        .name("waylan-screen-saver-listener".to_string())
        .spawn(move || {
            while !coordinator.is_shutting_down() {
                let Ok(conn) = Connection::session() else {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                };
                register_screen_saver_rules(&conn);
                // 连接建立或重连后主动同步一次当前真实锁屏状态，避免休眠唤醒期间信号丢失
                let active = query_initial_screen_saver_active(&conn);
                if screen_locked_clone.swap(active, Ordering::AcqRel) != active {
                    coordinator.wake();
                }
                let iter = MessageIterator::from(&conn);
                for msg_res in iter {
                    if coordinator.is_shutting_down() {
                        break;
                    }
                    let Ok(msg) = msg_res else {
                        break;
                    };
                    if handle_screen_saver_signal(&msg, &screen_locked_clone) {
                        coordinator.wake();
                    }
                }
                if coordinator.is_shutting_down() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;
    use waylan_core::LoopWakeup;

    #[test]
    fn test_screen_saver_listener_lifecycle() {
        let wakeup = LoopWakeup::new().ok();
        let exit_flag = Arc::new(AtomicBool::new(false));
        let coordinator = IbusShutdownCoordinator::new(Arc::clone(&exit_flag), wakeup, None);

        let (locked_flag, mut listener) = spawn_screen_saver_listener(false, coordinator);
        assert!(!locked_flag.load(Ordering::Acquire));

        // 稍等 50ms 确保监听线程完成 D-Bus 会话连接与 MatchRule 注册
        thread::sleep(Duration::from_millis(50));
        exit_flag.store(true, Ordering::Release);

        // 停止监听器并安全回收线程
        listener.stop();
    }
}
