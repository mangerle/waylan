//! IBus 全局引擎变更被动监听器
//!
//! # 设计原理
//! - **实现初衷**：在 Polkit 弹窗关闭、特定窗口切换等场景下，GNOME Shell / IBus 可能将引擎异常切为
//!   `xkb:us::eng`。若仅依赖定时轮询心跳（1000ms），用户在打字期间会感受到明显的滞后卡顿。
//! - **核心优势**：通过在 IBus 私有总线上被动监听 `GlobalEngineChanged` 信号，在引擎发生偏离的数毫秒内
//!   即刻唤醒主监控循环执行自愈，彻底消除状态撕裂的感知延迟。
//! - **代价与局限**：独立监听线程常驻阻塞于 D-Bus 消息读取，通过 RAII 句柄在退场时广播信号安全回收。

use std::thread::{Builder as ThreadBuilder, JoinHandle};
use std::time::Duration;

use log::{debug, warn};
use zbus::blocking::connection::Builder;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Message;

use crate::IbusShutdownCoordinator;

/// 后台常驻引擎变更信号监听器 RAII 句柄，Drop 时自动唤醒并回收工作线程
pub(crate) struct EngineChangeListenerHandle {
    ibus_addr: String,
    join_handle: Option<JoinHandle<()>>,
}

impl EngineChangeListenerHandle {
    /// 停止后台引擎信号监听线程并回收其句柄
    pub(crate) fn stop(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            // 向 IBus 总线广播一条信号以即时唤醒阻塞在 iter.next() 的监听线程
            if let Ok(conn) = Builder::address(self.ibus_addr.as_str()).and_then(|b| b.build()) {
                let _ = conn.emit_signal(
                    None::<&str>,
                    "/org/freedesktop/IBus",
                    "org.freedesktop.IBus",
                    "GlobalEngineChanged",
                    &("waylan",),
                );
            }
            let _ = handle.join();
        }
    }
}

impl Drop for EngineChangeListenerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 注册 IBus 全局引擎变更信号匹配规则
fn register_engine_change_rules(conn: &Connection) {
    let rule = "type='signal',interface='org.freedesktop.IBus',member='GlobalEngineChanged'";
    let _ = conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "AddMatch",
        &rule,
    );
}

/// 解析全局引擎变更信号并提取新引擎名称
pub(crate) fn parse_engine_change_signal(msg: &Message) -> Option<String> {
    let header = msg.header();
    let member = header.member().map(|m| m.as_str());
    if member == Some("GlobalEngineChanged") {
        if let Ok(name) = msg.body().deserialize::<String>() {
            return Some(name);
        }
    }
    None
}

/// 引擎信号被动监听线程主体
fn run_engine_change_listener(ibus_addr: String, coordinator: IbusShutdownCoordinator) {
    while !coordinator.is_shutting_down() {
        let Ok(conn) = Builder::address(ibus_addr.as_str()).and_then(|b| b.build()) else {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };
        register_engine_change_rules(&conn);

        let iter = MessageIterator::from(&conn);
        for msg_res in iter {
            if coordinator.is_shutting_down() {
                break;
            }
            let Ok(msg) = msg_res else {
                break;
            };
            if let Some(engine_name) = parse_engine_change_signal(&msg) {
                if engine_name != "waylan" {
                    debug!(
                        "[微澜 IBus] 监听到全局引擎变更为 [{}]，即刻唤醒自愈调度",
                        engine_name
                    );
                    coordinator.wake();
                }
            }
        }
        if coordinator.is_shutting_down() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 启动后台常驻 IBus 全局引擎变更信号监听器
pub(crate) fn spawn_engine_change_listener(
    ibus_addr: &str,
    coordinator: IbusShutdownCoordinator,
) -> EngineChangeListenerHandle {
    let addr_clone = ibus_addr.to_string();
    let addr_thread = ibus_addr.to_string();
    let handle = ThreadBuilder::new()
        .name("waylan-engine-listener".to_string())
        .spawn(move || run_engine_change_listener(addr_thread, coordinator))
        .map_err(|e| warn!("[微澜 IBus 警告] 创建引擎变更信号监听线程失败: {}", e))
        .ok();

    EngineChangeListenerHandle {
        ibus_addr: addr_clone,
        join_handle: handle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use waylan_sys::LoopWakeup;

    #[test]
    fn test_engine_change_listener_lifecycle() {
        let wakeup = LoopWakeup::new().ok();
        let exit_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let coordinator = IbusShutdownCoordinator::new(Arc::clone(&exit_flag), wakeup, None);

        // 使用无效地址验证容错重连与安全停机
        let mut listener =
            spawn_engine_change_listener("unix:path=/tmp/non_existent.sock", coordinator);
        thread::sleep(Duration::from_millis(30));
        exit_flag.store(true, std::sync::atomic::Ordering::Release);
        listener.stop();
    }

    #[test]
    fn test_parse_engine_change_signal_success() {
        let msg = Message::signal(
            "/org/freedesktop/IBus",
            "org.freedesktop.IBus",
            "GlobalEngineChanged",
        )
        .unwrap()
        .build(&("xkb:us::eng",))
        .unwrap();
        let parsed = parse_engine_change_signal(&msg);
        assert_eq!(parsed.as_deref(), Some("xkb:us::eng"));
    }

    #[test]
    fn test_parse_engine_change_signal_irrelevant() {
        let msg = Message::signal(
            "/org/freedesktop/IBus",
            "org.freedesktop.IBus",
            "OtherSignal",
        )
        .unwrap()
        .build(&("waylan",))
        .unwrap();
        let parsed = parse_engine_change_signal(&msg);
        assert!(parsed.is_none());
    }
}
