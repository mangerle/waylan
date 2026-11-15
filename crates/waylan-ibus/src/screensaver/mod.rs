//! 桌面锁屏状态被动感知与无阻塞自愈模块
//!
//! # 设计原理
//! - **实现初衷**：在锁屏状态下自动直通按键并禁用候选框与中文录入，防止密码泄露与锁屏界面误触发。
//! - **核心优势**：
//!   1. 热路径（`process_key_event` / `focus_in`）通过 `SystemScreenSaverQuery` 纯内存读取原子标志
//!      `Arc<AtomicBool>`，彻底消除持有核心互斥锁期间的同步 D-Bus 建连与 RPC 阻塞；
//!   2. 后台监听线程同时订阅 Session 总线（`ScreenSaver`）与 System 总线（`systemd-logind`）；
//!   3. 极端兜底探测通过有界通道非阻塞 `try_send` 异步触发，绝不拖慢输入响应。
//! - **代价与局限**：极端信号丢失场景下自愈存在单次异步探测的毫秒级延迟。

mod dbus;
mod listener;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use log::info;
use waylan_sys::LoopWakeup;

pub(crate) use dbus::query_current_screen_saver_active;
#[cfg(test)]
pub(crate) use listener::spawn_screen_saver_listener_with_flag;
pub(crate) use listener::spawn_screen_saver_listener_with_probe;

/// 桌面锁屏状态查询抽象 Trait，解耦 D-Bus 真实调用并支持单元测试
pub(crate) trait ScreenLockQuery: Send + Sync {
    /// 查询当前系统是否处于锁屏或屏幕保护状态（热路径必须保持无阻塞）
    fn is_screen_locked(&self) -> bool;

    /// 非阻塞请求后台异步刷新一次权威锁屏状态（默认无操作）
    fn request_async_probe(&self) {}
}

/// 系统默认锁屏状态查询实现
///
/// 当绑定了后台被动监听维护的 `screen_locked_flag` 时，热路径仅执行纯内存原子读，
/// 彻底消除按键与焦点切换路径上的同步 D-Bus 建连与 RPC 阻塞。
#[derive(Clone, Debug, Default)]
pub(crate) struct SystemScreenSaverQuery {
    screen_locked_flag: Option<Arc<AtomicBool>>,
    probe_tx: Option<SyncSender<()>>,
}

impl SystemScreenSaverQuery {
    /// 创建仅绑定被动监听原子标志的系统锁屏查询器
    #[cfg(test)]
    pub(crate) fn with_flag(flag: Arc<AtomicBool>) -> Self {
        Self {
            screen_locked_flag: Some(flag),
            probe_tx: None,
        }
    }

    /// 创建绑定被动监听原子标志与非阻塞异步探测触发通道的查询器
    pub(crate) fn with_flag_and_probe(flag: Arc<AtomicBool>, probe_tx: SyncSender<()>) -> Self {
        Self {
            screen_locked_flag: Some(flag),
            probe_tx: Some(probe_tx),
        }
    }
}

impl ScreenLockQuery for SystemScreenSaverQuery {
    fn is_screen_locked(&self) -> bool {
        if let Some(ref flag) = self.screen_locked_flag {
            // 热路径仅读取由 Session + logind 监听线程维护的共享原子标志，零 I/O 零阻塞
            flag.load(Ordering::Acquire)
        } else {
            query_current_screen_saver_active()
        }
    }

    fn request_async_probe(&self) {
        if let Some(ref tx) = self.probe_tx {
            // 有界通道容量为 1，满时静默丢弃，天然合并高频触发
            let _ = tx.try_send(());
        }
    }
}

/// 锁屏状态残留自愈器
pub(crate) struct ScreenLockHealer<Q = SystemScreenSaverQuery> {
    last_check_ms: AtomicU64,
    throttle_ms: u64,
    query: Q,
}

impl Default for ScreenLockHealer<SystemScreenSaverQuery> {
    fn default() -> Self {
        Self::new(SystemScreenSaverQuery::default())
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

    /// 查询当前系统是否处于锁屏状态（纯内存原子读）
    #[inline]
    pub(crate) fn is_screen_locked(&self) -> bool {
        self.query.is_screen_locked()
    }

    /// 按键级锁屏残留即时自愈：
    /// 当内部状态仍处于锁屏直通模式时，按节流周期非阻塞触发后台异步核实，
    /// 并基于最新原子状态即时解除直通保护。
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
                self.query.request_async_probe();
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
    /// 触发非阻塞异步刷新并依据原子状态决定是否解除锁屏直通
    pub(crate) fn heal_on_focus(
        &self,
        is_password_context: bool,
        is_screen_locked: &mut bool,
        loop_wakeup: Option<&LoopWakeup>,
    ) -> bool {
        if !is_password_context && *is_screen_locked {
            self.query.request_async_probe();
            if !self.query.is_screen_locked() {
                info!("[微澜 IBus] 普通窗口获焦且系统已解锁，自动校准并解除残留的锁屏直通保护。");
                *is_screen_locked = false;
                if let Some(wakeup) = loop_wakeup {
                    wakeup.wake();
                }
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IbusShutdownCoordinator;
    use std::sync::mpsc::sync_channel;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_screen_saver_listener_lifecycle() {
        let wakeup = LoopWakeup::new().ok();
        let exit_flag = Arc::new(AtomicBool::new(false));
        let coordinator = IbusShutdownCoordinator::new(Arc::clone(&exit_flag), wakeup, None);

        let locked_flag = Arc::new(AtomicBool::new(false));
        let mut listener =
            spawn_screen_saver_listener_with_flag(Arc::clone(&locked_flag), coordinator);
        assert!(!locked_flag.load(Ordering::Acquire));

        thread::sleep(Duration::from_millis(50));
        exit_flag.store(true, Ordering::Release);
        listener.stop();
    }

    #[test]
    fn test_system_query_reads_flag_and_requests_probe_nonblocking() {
        let flag = Arc::new(AtomicBool::new(true));
        let (tx, rx) = sync_channel::<()>(1);
        let query = SystemScreenSaverQuery::with_flag_and_probe(Arc::clone(&flag), tx);

        assert!(query.is_screen_locked());
        query.request_async_probe();
        // 连续调用两次，容量为 1 不应阻塞且第二条静默合并
        query.request_async_probe();
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());

        flag.store(false, Ordering::Release);
        assert!(!query.is_screen_locked());
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

        let mut locked = true;
        let healed = healer.heal_on_focus(true, &mut locked, None);
        assert!(!healed);
        assert!(locked);

        let healed = healer.heal_on_focus(false, &mut locked, None);
        assert!(healed);
        assert!(!locked);

        let healed = healer.heal_on_focus(false, &mut locked, None);
        assert!(!healed);
        assert!(!locked);

        healer.query().is_locked.store(true, Ordering::Release);
        let mut locked = true;
        let healed = healer.heal_on_focus(false, &mut locked, None);
        assert!(!healed);
        assert!(locked, "系统实际处于锁屏时绝不允许误解除锁屏直通");
    }

    #[test]
    fn test_screen_lock_healer_on_key() {
        let query = MockScreenQuery {
            is_locked: AtomicBool::new(false),
        };
        let healer = ScreenLockHealer::with_throttle(query, 50);

        let mut locked = true;
        let healed = healer.maybe_heal_on_key(true, &mut locked, None);
        assert!(!healed);
        assert!(locked);

        let healed = healer.maybe_heal_on_key(false, &mut locked, None);
        assert!(healed);
        assert!(!locked);

        let healed = healer.maybe_heal_on_key(false, &mut locked, None);
        assert!(!healed);

        healer.query().is_locked.store(true, Ordering::Release);
        thread::sleep(Duration::from_millis(60));
        let mut locked = true;
        let healed = healer.maybe_heal_on_key(false, &mut locked, None);
        assert!(!healed);
        assert!(locked);
    }
}
