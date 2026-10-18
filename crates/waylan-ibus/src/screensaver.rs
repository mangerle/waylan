//! 桌面锁屏状态被动感知与信号监听
//!
//! # 设计原理
//! - **实现初衷**：在锁屏状态下必须自动直通按键并禁用候选框与中文录入，防止密码泄露与锁屏界面误触发。
//! - **核心优势**：通过 D-Bus 信号被动订阅（MatchRule）监听锁屏变迁，彻底杜绝在主循环中高频主动轮询同步 RPC。
//! - **代价与局限**：监听线程在 Drop 时通过向总线发射本地信号唤醒以安全回收线程。

use crate::IbusShutdownCoordinator;
use log::info;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{Builder as ThreadBuilder, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use waylan_core::LoopWakeup;
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

/// 快速探测当前系统锁屏状态（自动获取用户会话连接）
pub(crate) fn query_current_screen_saver_active() -> bool {
    Connection::session()
        .ok()
        .as_ref()
        .map(query_initial_screen_saver_active)
        .unwrap_or(false)
}

/// 桌面锁屏状态查询抽象 Trait，解耦 D-Bus 真实调用并支持单元测试
pub(crate) trait ScreenLockQuery: Send + Sync {
    /// 查询当前系统是否处于锁屏或屏幕保护状态
    fn is_screen_locked(&self) -> bool;
}

/// 系统默认锁屏状态查询实现（通过 D-Bus 会话总线探测）
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemScreenSaverQuery;

impl ScreenLockQuery for SystemScreenSaverQuery {
    fn is_screen_locked(&self) -> bool {
        query_current_screen_saver_active()
    }
}

/// 锁屏状态残留自愈器
///
/// # 设计原理
/// - **实现初衷**：在极端场景下（如休眠唤醒、信号时序颠倒），输入法可能滞留在锁屏直通模式，
///   本组件封装按键级与焦点级的锁屏状态检测与自愈逻辑，避免锁屏检测细节污染输入法核心服务。
/// - **核心优势**：通过时间戳节流避免高频按键下重复触发同步 D-Bus 调用，并在普通窗口获焦时即时复位。
/// - **代价与局限**：自愈探测依赖外部查询器实现。
pub(crate) struct ScreenLockHealer<Q = SystemScreenSaverQuery> {
    last_check_ms: AtomicU64,
    throttle_ms: u64,
    query: Q,
}

impl Default for ScreenLockHealer<SystemScreenSaverQuery> {
    fn default() -> Self {
        Self::new(SystemScreenSaverQuery)
    }
}

impl<Q: ScreenLockQuery> ScreenLockHealer<Q> {
    /// 默认按键探测防抖节流间隔（毫秒）
    pub(crate) const DEFAULT_THROTTLE_MS: u64 = 500;

    /// 创建带有指定查询器的自愈器
    pub(crate) fn new(query: Q) -> Self {
        Self::with_throttle(query, Self::DEFAULT_THROTTLE_MS)
    }

    /// 创建带有指定查询器与防抖节流周期的自愈器
    pub(crate) fn with_throttle(query: Q, throttle_ms: u64) -> Self {
        Self {
            last_check_ms: AtomicU64::new(0),
            throttle_ms,
            query,
        }
    }

    /// 获取底层查询器的只读借用引用
    #[cfg(test)]
    pub(crate) fn query(&self) -> &Q {
        &self.query
    }

    /// 按键级锁屏残留即时自愈：
    /// 当当前窗口为普通输入上下文，但内部状态仍处于锁屏直通模式时，
    /// 节流探测桌面真实锁屏状态。若系统实际已解锁，立即解除直通保护并唤醒主循环。
    pub(crate) fn maybe_heal_on_key(
        &self,
        is_password_context: bool,
        is_screen_locked: &mut bool,
        loop_wakeup: Option<&LoopWakeup>,
    ) -> bool {
        if !is_password_context && *is_screen_locked {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let last_ms = self.last_check_ms.load(Ordering::Acquire);
            if now_ms.saturating_sub(last_ms) >= self.throttle_ms {
                self.last_check_ms.store(now_ms, Ordering::Release);
                if !self.query.is_screen_locked() {
                    info!("[微澜 IBus] 按键事件探测到系统已解锁，即时校准解除锁屏直通。");
                    *is_screen_locked = false;
                    if let Some(wakeup) = loop_wakeup {
                        wakeup.wake();
                    }
                    return true;
                }
            }
        }
        false
    }

    /// 普通窗口获焦时的即时自愈：
    /// 若当前获焦的并非密码框，而内部仍记录为锁屏状态，说明桌面已解锁但锁屏信号丢失，自动解除锁屏直通
    pub(crate) fn heal_on_focus(
        &self,
        is_password_context: bool,
        is_screen_locked: &mut bool,
        loop_wakeup: Option<&LoopWakeup>,
    ) -> bool {
        if !is_password_context && *is_screen_locked {
            info!("[微澜 IBus] 普通窗口获焦，自动校准并解除残留的锁屏直通保护。");
            *is_screen_locked = false;
            if let Some(wakeup) = loop_wakeup {
                wakeup.wake();
            }
            return true;
        }
        false
    }
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

    struct MockScreenQuery {
        is_locked: AtomicBool,
    }

    impl ScreenLockQuery for MockScreenQuery {
        fn is_screen_locked(&self) -> bool {
            self.is_locked.load(Ordering::Acquire)
        }
    }

    #[test]
    fn test_screen_lock_healer_focus_in() {
        let healer = ScreenLockHealer::new(MockScreenQuery {
            is_locked: AtomicBool::new(false),
        });

        // 1. 密码框获焦：不解除锁屏直通
        let mut locked = true;
        let healed = healer.heal_on_focus(true, &mut locked, None);
        assert!(!healed);
        assert!(locked);

        // 2. 普通窗口获焦：即时自愈解除锁屏直通
        let healed = healer.heal_on_focus(false, &mut locked, None);
        assert!(healed);
        assert!(!locked);

        // 3. 正常未锁屏状态获焦：无需自愈
        let healed = healer.heal_on_focus(false, &mut locked, None);
        assert!(!healed);
        assert!(!locked);
    }

    #[test]
    fn test_screen_lock_healer_on_key() {
        let query = MockScreenQuery {
            is_locked: AtomicBool::new(false),
        };
        let healer = ScreenLockHealer::with_throttle(query, 50);

        // 1. 密码框按键：不触发自愈
        let mut locked = true;
        let healed = healer.maybe_heal_on_key(true, &mut locked, None);
        assert!(!healed);
        assert!(locked);

        // 2. 普通窗口按键，系统已解锁：成功自愈
        let healed = healer.maybe_heal_on_key(false, &mut locked, None);
        assert!(healed);
        assert!(!locked);

        // 3. 再次按键，已在非锁屏态：不触发自愈
        let healed = healer.maybe_heal_on_key(false, &mut locked, None);
        assert!(!healed);

        // 4. 系统仍处于锁屏状态时：不自愈
        healer.query().is_locked.store(true, Ordering::Release);
        thread::sleep(Duration::from_millis(60));
        let mut locked = true;
        let healed = healer.maybe_heal_on_key(false, &mut locked, None);
        assert!(!healed);
        assert!(locked);
    }
}
