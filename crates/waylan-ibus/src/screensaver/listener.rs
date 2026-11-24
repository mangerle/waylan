//! 后台常驻锁屏信号监听与异步探测线程管理
//!
//! # 设计原理
//! - **实现初衷**：同时监听 Session 总线（`ScreenSaver.ActiveChanged`）与 System 总线
//!   （`systemd-logind` 的 `Lock`/`Unlock`/`LockedHint`），并承接热路径发来的非阻塞探测请求，
//!   将权威锁屏状态实时同步至共享原子标志 `Arc<AtomicBool>`。
//! - **核心优势**：
//!   1. 彻底消除 `process_key_event` 与 `focus_in` 热路径上的同步 D-Bus 建连与 RPC 阻塞；
//!   2. 监听线程退出时通过广播唤醒与 `Connection::close()` 即时中断阻塞迭代，停机无卡死。
//! - **代价与局限**：需维护最多三个轻量常驻监听/探测线程句柄并在 `Drop` 时统一回收。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::thread::{Builder as ThreadBuilder, JoinHandle};
use std::time::Duration;

use futures_util::StreamExt;
use log::warn;
use parking_lot::Mutex;
use zbus::MessageStream;
use zbus::connection::Connection;

use super::dbus::{
    handle_session_screen_saver_signal, handle_system_logind_signal,
    query_current_screen_saver_active, query_initial_screen_saver_active,
    register_session_screen_saver_rules, register_system_logind_rules,
};
use crate::IbusShutdownCoordinator;

/// 后台常驻锁屏信号监听器 RAII 句柄，Drop 时自动唤醒并回收所有工作线程
pub(crate) struct ScreenSaverListenerHandle {
    session_handle: Option<JoinHandle<()>>,
    system_handle: Option<JoinHandle<()>>,
    probe_handle: Option<JoinHandle<()>>,
    session_conn: Arc<Mutex<Option<Connection>>>,
    system_conn: Arc<Mutex<Option<Connection>>>,
}

impl ScreenSaverListenerHandle {
    /// 停止所有后台锁屏监听与探测线程并回收句柄
    pub(crate) fn stop(&mut self) {
        // 1. 主动关闭 Session 总线连接以即时中断 ScreenSaver MessageStream，避免广播虚假信号
        if let Some(conn) = self.session_conn.lock().take() {
            let _ = zbus::block_on(async { conn.close().await });
        }
        if let Some(handle) = self.session_handle.take() {
            let _ = handle.join();
        }

        // 2. 主动关闭 System 总线连接以即时中断 logind MessageStream
        if let Some(conn) = self.system_conn.lock().take() {
            let _ = zbus::block_on(async { conn.close().await });
        }
        if let Some(handle) = self.system_handle.take() {
            let _ = handle.join();
        }

        // 3. 回收异步探测线程（由调用方在停机前 drop sender，或等待 recv_timeout 感知停机标志退出）
        if let Some(handle) = self.probe_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ScreenSaverListenerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Session 总线锁屏信号被动监听线程主体
fn run_session_screen_saver_listener(
    screen_locked: Arc<AtomicBool>,
    coordinator: IbusShutdownCoordinator,
    conn_slot: Arc<Mutex<Option<Connection>>>,
) {
    while !coordinator.is_shutting_down() {
        let Ok(conn) = zbus::block_on(async { Connection::session().await }) else {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };
        if coordinator.is_shutting_down() {
            break;
        }
        *conn_slot.lock() = Some(conn.clone());
        register_session_screen_saver_rules(&conn);

        // 连接建立或重连后立即对齐当前真实锁屏状态（包含 logind 与 ScreenSaver）
        let active =
            query_current_screen_saver_active() || query_initial_screen_saver_active(&conn);
        if screen_locked.swap(active, Ordering::AcqRel) != active {
            coordinator.wake();
        }

        zbus::block_on(async {
            let mut stream = MessageStream::from(&conn);
            while let Some(msg_res) = stream.next().await {
                if coordinator.is_shutting_down() {
                    break;
                }
                let Ok(msg) = msg_res else {
                    break;
                };
                if handle_session_screen_saver_signal(&msg, &screen_locked) {
                    coordinator.wake();
                }
            }
        });
        *conn_slot.lock() = None;

        if coordinator.is_shutting_down() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// System 总线 `systemd-logind` 锁屏信号被动监听线程主体
fn run_system_logind_listener(
    screen_locked: Arc<AtomicBool>,
    coordinator: IbusShutdownCoordinator,
    conn_slot: Arc<Mutex<Option<Connection>>>,
) {
    while !coordinator.is_shutting_down() {
        let Ok(conn) = zbus::block_on(async { Connection::system().await }) else {
            std::thread::sleep(Duration::from_millis(1000));
            continue;
        };
        if coordinator.is_shutting_down() {
            break;
        }
        *conn_slot.lock() = Some(conn.clone());
        register_system_logind_rules(&conn);

        zbus::block_on(async {
            let mut stream = MessageStream::from(&conn);
            while let Some(msg_res) = stream.next().await {
                if coordinator.is_shutting_down() {
                    break;
                }
                let Ok(msg) = msg_res else {
                    break;
                };
                if handle_system_logind_signal(&msg, &screen_locked) {
                    coordinator.wake();
                }
            }
        });
        *conn_slot.lock() = None;

        if coordinator.is_shutting_down() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// 后台异步锁屏探测工作线程主体：响应热路径非阻塞 `try_send` 探测请求并更新原子标志
fn run_async_probe_worker(
    probe_rx: Receiver<()>,
    screen_locked: Arc<AtomicBool>,
    coordinator: IbusShutdownCoordinator,
) {
    while !coordinator.is_shutting_down() {
        match probe_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(()) => {
                // 清空积压的冗余探测请求，仅执行一次权威状态查询
                while probe_rx.try_recv().is_ok() {}
                if coordinator.is_shutting_down() {
                    break;
                }
                let active = query_current_screen_saver_active();
                if screen_locked.swap(active, Ordering::AcqRel) != active {
                    coordinator.wake();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// 启动后台常驻 D-Bus 锁屏信号被动监听与异步探测线程组
pub(crate) fn spawn_screen_saver_listener_with_probe(
    screen_locked: Arc<AtomicBool>,
    coordinator: IbusShutdownCoordinator,
    probe_rx: Option<Receiver<()>>,
) -> ScreenSaverListenerHandle {
    let session_conn = Arc::new(Mutex::new(None));
    let session_flag = Arc::clone(&screen_locked);
    let session_coord = coordinator.clone();
    let session_conn_slot = Arc::clone(&session_conn);
    let session_handle = ThreadBuilder::new()
        .name("waylan-screen-saver-listener".to_string())
        .spawn(move || {
            run_session_screen_saver_listener(session_flag, session_coord, session_conn_slot)
        })
        .map_err(|e| warn!("[微澜 IBus 警告] 创建会话锁屏信号监听线程失败: {}", e))
        .ok();

    let system_conn = Arc::new(Mutex::new(None));
    let system_flag = Arc::clone(&screen_locked);
    let system_coord = coordinator.clone();
    let conn_slot = Arc::clone(&system_conn);
    let system_handle = ThreadBuilder::new()
        .name("waylan-logind-listener".to_string())
        .spawn(move || run_system_logind_listener(system_flag, system_coord, conn_slot))
        .map_err(|e| warn!("[微澜 IBus 警告] 创建 logind 锁屏信号监听线程失败: {}", e))
        .ok();

    let probe_handle = probe_rx.and_then(|rx| {
        let probe_flag = Arc::clone(&screen_locked);
        let probe_coord = coordinator;
        ThreadBuilder::new()
            .name("waylan-screen-lock-probe".to_string())
            .spawn(move || run_async_probe_worker(rx, probe_flag, probe_coord))
            .map_err(|e| warn!("[微澜 IBus 警告] 创建异步锁屏探测线程失败: {}", e))
            .ok()
    });

    ScreenSaverListenerHandle {
        session_handle,
        system_handle,
        probe_handle,
        session_conn,
        system_conn,
    }
}

/// 基于指定原子锁屏状态标志启动后台常驻 D-Bus 锁屏信号被动监听器（不挂载外部探测通道）
#[cfg(test)]
pub(crate) fn spawn_screen_saver_listener_with_flag(
    screen_locked: Arc<AtomicBool>,
    coordinator: IbusShutdownCoordinator,
) -> ScreenSaverListenerHandle {
    spawn_screen_saver_listener_with_probe(screen_locked, coordinator, None)
}
