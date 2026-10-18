//! IBus 宿主事件监控、语音流式刷新与配置热重载事件循环
//!
//! # 设计原理
//! - **实现初衷**：在单线程阻塞事件循环中，统筹管理 inotify 配置文件变化感知、跨线程 eventfd 唤醒、
//!   语音实时流式识别上屏以及系统锁屏状态同步。
//! - **核心优势**：通过 poll 机制实现零忙轮询，兼顾流式语音的低延迟刷新（60ms 轮询窗口）与空闲时极低 CPU 消耗。
//! - **代价与局限**：主线程常驻阻塞于 poll 调用。

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use log::{debug, info};
use rustix::event::{poll, PollFd, PollFlags};
use waylan_core::fs_watcher::{check_reload_requested_or_files_changed, InotifyWatcher};
use waylan_core::CandidateIndexStyle;
use waylan_core::LoopWakeup;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::engine::{UiSnapshot, WAYLAN_ENGINE_OBJECT_PATH};
use super::keys::IbusSessionCore;
use super::screensaver::{query_initial_screen_saver_active, spawn_screen_saver_listener};
use super::types::{
    build_ibus_lookup_table, build_ibus_preedit_text, build_ibus_text, build_input_mode_property,
    IBUS_ENGINE_PREEDIT_CLEAR, IBUS_ORIENTATION_HORIZONTAL,
};
use crate::{IbusConfigReloader, IbusShutdownCoordinator};

/// 后台监控循环参数结构体
pub(crate) struct IbusMonitorParams {
    pub(crate) watched_paths: Vec<PathBuf>,
    pub(crate) last_mtime: Option<SystemTime>,
    pub(crate) reload_requested: Option<Arc<AtomicBool>>,
    pub(crate) coordinator: IbusShutdownCoordinator,
    pub(crate) config_reloader: Option<Arc<dyn IbusConfigReloader>>,
}

/// 处理锁屏状态变迁与输入法状态同步
fn handle_screen_saver_transition(
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
        if let Ok(mut guard) = shared_core.lock() {
            guard.is_screen_locked = true;
            guard.reset_context();
        }
    } else {
        info!("[微澜 IBus] 检测到系统屏幕已解锁，正在自动校准并切回微澜输入法。");
        if let Ok(mut guard) = shared_core.lock() {
            guard.is_screen_locked = false;
            guard.is_password_context = false;
        }
        ensure_global_engine_active(conn);
        let _ = Command::new("gsettings")
            .args(["set", "org.gnome.desktop.input-sources", "current", "0"])
            .output();
    }
    *last_screen_locked = current_screen_locked;
}

/// 获取当前语音活跃状态与目标 IBus 引擎对象路径
fn query_voice_active_and_path(shared_core: &Arc<Mutex<IbusSessionCore>>) -> (bool, String) {
    if let Ok(guard) = shared_core.lock() {
        let active = guard.is_voice_active();
        let path = guard
            .active_engine_path
            .clone()
            .unwrap_or_else(|| WAYLAN_ENGINE_OBJECT_PATH.to_string());
        (active, path)
    } else {
        (false, WAYLAN_ENGINE_OBJECT_PATH.to_string())
    }
}

/// 发送语音波形候选框与实时预编辑文本更新
fn emit_voice_stream_update(conn: &Connection, target_engine_path: &str, voice_snap: &UiSnapshot) {
    let table_visible = !voice_snap.candidates.is_empty();
    if let Ok(table_val) = build_ibus_lookup_table(
        &voice_snap.candidates,
        voice_snap.page_size,
        voice_snap.selected_index,
        voice_snap.orientation,
        voice_snap.index_style,
    ) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdateLookupTable",
            &(Value::from(table_val), table_visible),
        );
    }

    if let Ok(preedit_val) = build_ibus_preedit_text(&voice_snap.preedit) {
        let char_len = voice_snap.preedit.chars().count() as u32;
        let visible = !voice_snap.preedit.is_empty();
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdatePreeditText",
            &(
                Value::from(preedit_val),
                char_len,
                visible,
                IBUS_ENGINE_PREEDIT_CLEAR,
            ),
        );
    }
}

/// 发射清空候选框与预编辑的信号
fn emit_voice_stream_exit(conn: &Connection, target_engine_path: &str) {
    if let Ok(empty_table) = build_ibus_lookup_table(
        &[],
        1,
        0,
        IBUS_ORIENTATION_HORIZONTAL,
        CandidateIndexStyle::None,
    ) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdateLookupTable",
            &(Value::from(empty_table), false),
        );
    }
    if let Ok(empty_preedit) = build_ibus_preedit_text("") {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "UpdatePreeditText",
            &(
                Value::from(empty_preedit),
                0u32,
                false,
                IBUS_ENGINE_PREEDIT_CLEAR,
            ),
        );
    }
}

/// 自动断句文字上屏提交
fn commit_voice_text(conn: &Connection, target_engine_path: &str, commit_text: &str) {
    if commit_text.is_empty() {
        return;
    }
    info!(
        "[微澜 IBus 语音] 自动断句文字上屏 (目标路径: {}): \"{}\"",
        target_engine_path, commit_text
    );
    emit_voice_stream_exit(conn, target_engine_path);
    if let Ok(text_val) = build_ibus_text(commit_text) {
        let _ = conn.emit_signal(
            None::<&str>,
            target_engine_path,
            "org.freedesktop.IBus.Engine",
            "CommitText",
            &(Value::from(text_val),),
        );
    }
}

/// 处理单轮语音事件轮询、流式更新与过渡退出
fn handle_voice_stream_cycle(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    target_engine_path: &str,
    is_voice_active: bool,
    last_voice_active: &mut bool,
) {
    if is_voice_active {
        let (voice_snap, voice_commit) = match shared_core.lock() {
            Ok(mut guard) => {
                let commit = guard.poll_voice_events();
                let snap = UiSnapshot::capture(&guard);
                (snap, commit)
            }
            Err(_) => return,
        };
        emit_voice_stream_update(conn, target_engine_path, &voice_snap);
        if let Some(ref commit_text) = voice_commit {
            commit_voice_text(conn, target_engine_path, commit_text);
        }
    } else if *last_voice_active {
        emit_voice_stream_exit(conn, target_engine_path);
    }
    *last_voice_active = is_voice_active;
}

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
            let mut guard = shared_core.lock().unwrap_or_else(|e| e.into_inner());
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
    let mode_changed = shared_core
        .lock()
        .ok()
        .and_then(|mut g| g.sync_shared_mode().map(|m| (m, g.is_caps_locked)));
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

/// 驱动后台文件热重载与托盘状态同步的阻塞监控循环
pub(crate) fn run_ibus_monitor_loop(
    conn: &Connection,
    shared_core: &Arc<Mutex<IbusSessionCore>>,
    inotify_watcher: &mut Option<InotifyWatcher>,
    mut params: IbusMonitorParams,
) {
    let mut startup_healing_rounds: u32 = 25;
    let initial_screen_locked = Connection::session()
        .ok()
        .as_ref()
        .map(query_initial_screen_saver_active)
        .unwrap_or(false);

    if initial_screen_locked {
        if let Ok(mut guard) = shared_core.lock() {
            guard.is_screen_locked = true;
        }
        info!("[微澜 IBus] 启动时检测到系统处于锁屏状态，已进入密码直通保护模式。");
    }

    let (screen_locked_flag, mut listener_handle) =
        spawn_screen_saver_listener(initial_screen_locked, params.coordinator.clone());
    let mut last_screen_locked = initial_screen_locked;
    let mut last_voice_active = false;
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

        let current_locked = screen_locked_flag.load(Ordering::Acquire);
        handle_screen_saver_transition(conn, shared_core, current_locked, &mut last_screen_locked);

        let (is_voice_active, target_engine_path) = query_voice_active_and_path(shared_core);
        let poll_timeout_ms = if is_voice_active { 60 } else { 1000 };
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
            super::session::extract_engine_name_from_desc_variant(&val)
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
            let _ = Command::new("gsettings")
                .args(["set", "org.gnome.desktop.input-sources", "current", "0"])
                .output();
        }
    }
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
        thread::sleep(Duration::from_millis(timeout_ms.max(50) as u64));
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
