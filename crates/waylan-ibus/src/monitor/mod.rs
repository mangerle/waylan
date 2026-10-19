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
use super::screensaver::{query_initial_screen_saver_active, spawn_screen_saver_listener};
use super::types::build_input_mode_property;
use crate::{IbusConfigReloader, IbusShutdownCoordinator};

/// 后台监控循环参数结构体
pub(crate) struct IbusMonitorParams {
    pub(crate) watched_paths: Vec<PathBuf>,
    pub(crate) last_mtime: Option<waylan_sys::FileSignature>,
    pub(crate) reload_requested: Option<Arc<AtomicBool>>,
    pub(crate) coordinator: IbusShutdownCoordinator,
    pub(crate) config_reloader: Option<Arc<dyn IbusConfigReloader>>,
}

mod lifecycle;
mod voice_stream;

pub(crate) use lifecycle::{ensure_global_engine_active, handle_screen_saver_transition};
pub(crate) use voice_stream::{handle_voice_stream_cycle, query_voice_active_and_path};

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

/// 处于锁屏保护期时的自愈探活与状态迁移处理
fn probe_and_handle_screensaver(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    screen_locked_flag: &AtomicBool,
    last_screen_locked: &mut bool,
    last_lock_probe_instant: &mut Instant,
) -> bool {
    let mut current_locked = screen_locked_flag.load(Ordering::Acquire);
    if (current_locked || *last_screen_locked)
        && last_lock_probe_instant.elapsed() >= Duration::from_secs(3)
    {
        *last_lock_probe_instant = Instant::now();
        let real_locked = Connection::session()
            .ok()
            .as_ref()
            .map(query_initial_screen_saver_active)
            .unwrap_or(false);
        if !real_locked && (current_locked || *last_screen_locked) {
            screen_locked_flag.store(false, Ordering::Release);
            current_locked = false;
        }
    }
    handle_screen_saver_transition(conn, shared_core, current_locked, last_screen_locked);
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
        -1
    }
}

/// 驱动后台文件热重载与托盘状态同步的阻塞监控循环
pub(crate) fn run_ibus_monitor_loop(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    mut params: IbusMonitorParams,
) {
    let mut startup_healing_rounds: u32 = 3;
    let initial_screen_locked = Connection::session()
        .ok()
        .as_ref()
        .map(query_initial_screen_saver_active)
        .unwrap_or(false);

    if initial_screen_locked {
        shared_core.lock().is_screen_locked = true;
        info!("[微澜 IBus] 启动时检测到系统处于锁屏状态，已进入密码直通保护模式。");
    }

    let (screen_locked_flag, mut listener_handle) =
        spawn_screen_saver_listener(initial_screen_locked, params.coordinator.clone());
    let mut last_screen_locked = initial_screen_locked;
    let mut last_voice_active = false;
    let mut last_lock_probe_instant = Instant::now();
    let mut last_reload_instant = Instant::now()
        .checked_sub(Duration::from_secs(10))
        .unwrap_or_else(Instant::now);

    loop {
        if params.coordinator.is_shutting_down() {
            info!("[微澜 IBus] 收到退出请求信号，正在安全退出 IBus 事件循环。");
            break;
        }

        if startup_healing_rounds > 0 {
            startup_healing_rounds = startup_healing_rounds.saturating_sub(1);
            ensure_global_engine_active(conn);
        }

        let current_locked = probe_and_handle_screensaver(
            conn,
            shared_core,
            &screen_locked_flag,
            &mut last_screen_locked,
            &mut last_lock_probe_instant,
        );

        let (is_voice_active, target_engine_path) = query_voice_active_and_path(shared_core);
        let poll_timeout_ms = compute_monitor_poll_timeout(
            is_voice_active,
            startup_healing_rounds,
            current_locked || last_screen_locked,
        );

        poll_wakeup_and_inotify(
            inotify_watcher.as_ref(),
            params.coordinator.loop_wakeup(),
            poll_timeout_ms,
        );

        handle_voice_stream_cycle(
            conn,
            shared_core,
            &target_engine_path,
            is_voice_active,
            &mut last_voice_active,
        );

        if params.coordinator.is_shutting_down() {
            info!("[微澜 IBus] 收到退出请求信号，正在安全退出 IBus 事件循环。");
            break;
        }

        let should_reload = check_reload_requested_or_files_changed(
            params.reload_requested.as_deref(),
            inotify_watcher,
            &params.watched_paths,
            &mut params.last_mtime,
        );
        if should_reload
            && handle_config_reload_event(shared_core, &mut params, &mut last_reload_instant)
        {
            break;
        }

        sync_engine_mode_property(conn, shared_core);
    }

    listener_handle.stop();
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
