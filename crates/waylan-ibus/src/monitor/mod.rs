//! IBus 宿主事件监控、语音流式刷新与配置热重载事件循环
//!
//! # 设计原理
//! - **实现初衷**：在单线程阻塞事件循环中，统筹管理 inotify 配置文件变化感知、跨线程 eventfd 唤醒、
//!   语音实时流式识别上屏以及系统锁屏状态同步。
//! - **核心优势**：通过 poll 机制实现零忙轮询，兼顾流式语音的低延迟刷新（60ms 轮询窗口）与空闲时极低 CPU 消耗。
//! - **代价与局限**：主线程常驻阻塞于 poll 调用。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, info};
use parking_lot::Mutex;
use rustix::event::{poll, PollFd, PollFlags};
use waylan_sys::fs_watcher::{check_reload_requested_or_files_changed, InotifyWatcher};
use waylan_sys::LoopWakeup;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::engine::WAYLAN_ENGINE_OBJECT_PATH;
use super::keys::IbusSessionCore;
use super::screensaver::{
    query_current_screen_saver_active, spawn_screen_saver_listener_with_probe,
};
use super::types::build_input_mode_property;
use crate::{IbusConfigReloader, IbusShutdownCoordinator};

/// 后台监控循环参数结构体
pub(crate) struct IbusMonitorParams {
    pub(crate) watched_paths: Vec<PathBuf>,
    pub(crate) last_mtime: Option<waylan_sys::FileSignature>,
    pub(crate) reload_requested: Option<Arc<AtomicBool>>,
    pub(crate) coordinator: IbusShutdownCoordinator,
    pub(crate) config_reloader: Option<Arc<dyn IbusConfigReloader>>,
    pub(crate) screen_locked_flag: Arc<AtomicBool>,
    pub(crate) probe_rx: Option<std::sync::mpsc::Receiver<()>>,
    pub(crate) ibus_address: String,
}

mod engine_listener;
mod lifecycle;
mod voice_stream;

pub(crate) use engine_listener::spawn_engine_change_listener;
pub(crate) use lifecycle::{ensure_global_engine_active, handle_screen_saver_transition};
pub(crate) use voice_stream::{
    handle_voice_stream_cycle, query_voice_active_and_path, VoiceStreamCycleParams,
};

/// 处理词库与配置热重载事件，若触发指示器形态变更自重启则返回 true
fn handle_config_reload_event(
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    params: &mut IbusMonitorParams,
    last_reload_instant: &mut Instant,
) -> bool {
    let now = Instant::now();
    if now.duration_since(*last_reload_instant) < Duration::from_millis(800) {
        debug!("[微澜 IBus] 处于热重载防抖窗口期内，已安全合并重复触发请求。");
        return false;
    }
    *last_reload_instant = now;

    if let Some(ref reloader) = params.config_reloader {
        let should_restart = {
            let mut guard = shared_core.lock();
            reloader.reload(&mut guard)
        };
        // 核心互斥锁 guard 已经彻底析构释放，在临界区外安全执行外部命令或 I/O，绝不阻塞打字按键
        reloader.post_reload();
        if should_restart {
            info!("[微澜 IBus] 热重载检测到指示器形态变更，正在触发平滑自重启...");
            params.coordinator.trigger_graceful_restart();
            return true;
        }
    }

    info!("[微澜 IBus] 已完成用户词库与运行配置热重载。");
    false
}

/// 同步托盘模式切换并发射 IBus 引擎 UpdateProperty 信号
fn sync_engine_mode_property(conn: &Connection, shared_core: &Arc<Mutex<IbusSessionCore>>) {
    let mode_changed = {
        let mut g = shared_core.lock();
        g.sync_shared_mode().map(|m| (m, g.is_caps_locked()))
    };
    if let Some((new_mode, is_caps)) = mode_changed {
        if let Ok(prop_val) = build_input_mode_property(new_mode, is_caps) {
            let _ = conn.emit_signal(
                Option::<&str>::None,
                WAYLAN_ENGINE_OBJECT_PATH,
                "org.freedesktop.IBus.Engine",
                "UpdateProperty",
                &(Value::from(prop_val),),
            );
            debug!("[微澜 IBus] 已同步托盘模式切换并发射 UpdateProperty 信号");
        }
    }
}

/// 锁屏探活与状态迁移处理参数包
///
/// # 设计原理
/// - **实现初衷**：锁屏探活需同时依赖总线连接、共享会话核心、锁屏共享标记以及两个跨轮次延续的可变槽位，
///   平铺参数会让调用点难以辨识可变状态，故收敛为专用的参数结构体。
/// - **核心优势**：跨轮次状态以 `&mut` 槽位形式显式承载，状态归属一目了然，杜绝位置错配导致的静默偏差。
/// - **代价与局限**：调用点需显式构造结构体字面量，代码量略增但语义更清晰。
struct ScreensaverProbeParams<'a> {
    /// 到 `ibus-daemon` 的阻塞连接
    conn: &'a Connection,
    /// 共享的输入法会话核心
    shared_core: &'a Arc<Mutex<IbusSessionCore>>,
    /// 系统锁屏状态共享标记
    screen_locked_flag: &'a AtomicBool,
    /// 上一轮观测到的锁屏状态（跨轮次延续的可变槽位）
    last_screen_locked: &'a mut bool,
    /// 上次锁屏状态探活时刻（跨轮次延续的可变槽位）
    last_lock_probe_instant: &'a mut Instant,
}

/// 处于锁屏保护期时的自愈探活与状态迁移处理
fn probe_and_handle_screensaver(params: &mut ScreensaverProbeParams<'_>) -> bool {
    let mut current_locked = params.screen_locked_flag.load(Ordering::Acquire);
    if params.last_lock_probe_instant.elapsed() >= Duration::from_secs(3) {
        *params.last_lock_probe_instant = Instant::now();
        let real_locked = query_current_screen_saver_active();
        if real_locked != current_locked {
            params
                .screen_locked_flag
                .store(real_locked, Ordering::Release);
            current_locked = real_locked;
        }
    }
    handle_screen_saver_transition(
        params.conn,
        params.shared_core,
        current_locked,
        &mut *params.last_screen_locked,
    );
    current_locked
}

/// 计算当前监控循环的 poll 等待超时策略（毫秒）
fn compute_monitor_poll_timeout(
    is_voice_active: bool,
    startup_healing_rounds: u32,
    is_locked: bool,
) -> i32 {
    if is_voice_active {
        60
    } else if startup_healing_rounds > 0 {
        100
    } else if is_locked {
        3000
    } else {
        1000
    }
}

/// 驱动后台文件热重载与托盘状态同步的阻塞监控循环
pub(crate) fn run_ibus_monitor_loop(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    mut params: IbusMonitorParams,
) {
    let initial_screen_locked = params.screen_locked_flag.load(Ordering::Acquire);

    if initial_screen_locked {
        shared_core.lock().is_screen_locked = true;
        info!("[微澜 IBus] 启动时检测到系统处于锁屏状态，已进入密码直通保护模式。");
    }

    let screen_locked_flag = Arc::clone(&params.screen_locked_flag);
    let mut listener_handle = spawn_screen_saver_listener_with_probe(
        Arc::clone(&screen_locked_flag),
        params.coordinator.clone(),
        params.probe_rx.take(),
    );
    let mut engine_listener_handle =
        spawn_engine_change_listener(&params.ibus_address, params.coordinator.clone());
    let mut state = MonitorLoopState::new(initial_screen_locked, screen_locked_flag);

    loop {
        let ctx = MonitorIterationContext { conn, shared_core };
        if !run_monitor_iteration(&ctx, inotify_watcher, &mut params, &mut state) {
            break;
        }
    }

    engine_listener_handle.stop();
    listener_handle.stop();
}

/// 监控循环跨轮次保留的可变状态
///
/// # 设计原理
/// - 将"需要在多轮事件循环之间延续"的状态（引擎自愈轮次、锁屏与语音状态快照、探活时刻）
///   收敛到单一结构体，避免监控主函数退化为超长参数列表。
struct MonitorLoopState {
    /// 启动期引擎自愈剩余轮次
    startup_healing_rounds: u32,
    /// 上一轮观测到的锁屏状态
    last_screen_locked: bool,
    /// 上一轮观测到的语音激活状态
    last_voice_active: bool,
    /// 上次锁屏状态探活时刻
    last_lock_probe_instant: Instant,
    /// 上次配置热重载触发时刻（用于防抖窗口判定）
    last_reload_instant: Instant,
    /// 系统锁屏状态共享标记
    screen_locked_flag: Arc<AtomicBool>,
}

impl MonitorLoopState {
    /// 构造监控循环的初始跨轮次状态
    fn new(initial_screen_locked: bool, screen_locked_flag: Arc<AtomicBool>) -> Self {
        Self {
            startup_healing_rounds: 3,
            last_screen_locked: initial_screen_locked,
            last_voice_active: false,
            last_lock_probe_instant: Instant::now(),
            last_reload_instant: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or_else(Instant::now),
            screen_locked_flag,
        }
    }
}

/// 单轮监控迭代所需的只读运行时上下文
struct MonitorIterationContext<'a> {
    /// 到 `ibus-daemon` 的阻塞连接
    conn: &'a Connection,
    /// 共享的输入法会话核心
    shared_core: &'a Arc<Mutex<IbusSessionCore>>,
}

/// 执行一轮监控迭代，返回是否应继续下一轮迭代
///
/// # 设计原理
/// - 单轮迭代严格遵循"退出检查 -> 轮询与语音阶段 -> 退出检查 -> 热重载阶段 -> 状态同步"的固定顺序，
///   任一阶段判定需要退场时立即结束事件循环。
fn run_monitor_iteration(
    ctx: &MonitorIterationContext<'_>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    params: &mut IbusMonitorParams,
    state: &mut MonitorLoopState,
) -> bool {
    if is_shutdown_requested(&params.coordinator) {
        return false;
    }

    if !monitor_poll_phase(ctx, inotify_watcher.as_ref(), params, state) {
        return false;
    }

    if is_shutdown_requested(&params.coordinator) {
        return false;
    }

    if monitor_config_reload_phase(ctx, inotify_watcher, params, state) {
        return false;
    }

    sync_engine_mode_property(ctx.conn, ctx.shared_core);
    true
}

/// 执行一轮监控的轮询阶段：启动期引擎自愈、锁屏探活、`poll` 等待与语音流式刷新
///
/// # 设计原理
/// - 返回 false 表示本阶段观测到退出请求，事件循环应尽快安全退场。
fn monitor_poll_phase(
    ctx: &MonitorIterationContext<'_>,
    inotify_watcher: Option<&InotifyWatcher>,
    params: &IbusMonitorParams,
    state: &mut MonitorLoopState,
) -> bool {
    // 启动期引擎自愈：确保会话初期全局引擎处于激活状态
    if state.startup_healing_rounds > 0 {
        state.startup_healing_rounds = state.startup_healing_rounds.saturating_sub(1);
        ensure_global_engine_active(ctx.conn);
    }

    let current_locked = probe_and_handle_screensaver(&mut ScreensaverProbeParams {
        conn: ctx.conn,
        shared_core: ctx.shared_core,
        screen_locked_flag: &state.screen_locked_flag,
        last_screen_locked: &mut state.last_screen_locked,
        last_lock_probe_instant: &mut state.last_lock_probe_instant,
    });

    let (is_voice_active, target_engine_path) = query_voice_active_and_path(ctx.shared_core);
    let poll_timeout_ms = compute_monitor_poll_timeout(
        is_voice_active,
        state.startup_healing_rounds,
        current_locked || state.last_screen_locked,
    );

    poll_wakeup_and_inotify(
        inotify_watcher,
        params.coordinator.loop_wakeup(),
        poll_timeout_ms,
    );

    handle_voice_stream_cycle(&mut VoiceStreamCycleParams {
        conn: ctx.conn,
        shared_core: ctx.shared_core,
        target_engine_path: target_engine_path.as_str(),
        is_voice_active,
        last_voice_active: &mut state.last_voice_active,
    });

    !is_shutdown_requested(&params.coordinator)
}

/// 执行一轮监控的配置热重载阶段，返回是否需要触发指示器形态变更自重启
fn monitor_config_reload_phase(
    ctx: &MonitorIterationContext<'_>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    params: &mut IbusMonitorParams,
    state: &mut MonitorLoopState,
) -> bool {
    let should_reload = check_reload_requested_or_files_changed(
        params.reload_requested.as_deref(),
        inotify_watcher,
        &params.watched_paths,
        &mut params.last_mtime,
    );
    should_reload
        && handle_config_reload_event(ctx.shared_core, params, &mut state.last_reload_instant)
}

/// 检查是否收到退出请求，并输出统一的退场日志
fn is_shutdown_requested(coordinator: &IbusShutdownCoordinator) -> bool {
    if coordinator.is_shutting_down() {
        info!("[微澜 IBus] 收到退出请求信号，正在安全退出 IBus 事件循环。");
        return true;
    }
    false
}

/// 使用 `poll` 监听 `inotify` 与跨线程 `eventfd` 唤醒描述符
fn poll_wakeup_and_inotify(
    inotify_watcher: Option<&InotifyWatcher>,
    loop_wakeup: Option<&LoopWakeup>,
    timeout_ms: i32,
) {
    let inotify_fd = inotify_watcher.map(|w| w.fd());
    let wakeup_fd = loop_wakeup.map(|w| w.fd());

    let mut fds: Vec<PollFd<'_>> = Vec::with_capacity(2);
    let wakeup_idx = if let Some(ref fd) = wakeup_fd {
        let idx = fds.len();
        fds.push(PollFd::new(&**fd, PollFlags::IN | PollFlags::ERR));
        Some(idx)
    } else {
        None
    };

    if let Some(ref fd) = inotify_fd {
        fds.push(PollFd::new(&**fd, PollFlags::IN | PollFlags::ERR));
    }

    if fds.is_empty() {
        let sleep_ms = if timeout_ms < 0 {
            100
        } else {
            timeout_ms.max(50) as u64
        };
        thread::sleep(Duration::from_millis(sleep_ms));
        return;
    }

    if let Ok(n) = poll(&mut fds, timeout_ms) {
        if n > 0 {
            if let Some(idx) = wakeup_idx {
                if fds[idx]
                    .revents()
                    .intersects(PollFlags::IN | PollFlags::ERR)
                {
                    if let Some(wakeup) = loop_wakeup {
                        wakeup.drain();
                    }
                }
            }
        }
    }
}
