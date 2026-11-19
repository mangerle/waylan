//! Wayland 输入法事件循环运行器与入口调度
//!
//! # 设计原理
//! - **实现初衷**：为 Wayland V1 与 V2 提供高层次的启动与循环派发入口，支持在 V2 不可用时无损降级回退。
//! - **核心优势**：通过模块化解耦，将协议绑定 (`globals.rs`) 与事件轮询抽象 (`poll.rs`) 分离，
//!   消除巨型单体文件，提高可维护性。
//! - **代价与局限**：降级接口需要返回携带 `Box<WaylanV2Session>` 的所有权元组。

mod globals;
mod poll;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use log::{info, warn};
use waylan_core::{AppConfig, SharedInputFlags, VoiceProvider};
use waylan_engine::AlgoEngine;
use waylan_sys::{FileSignature, LoopWakeup};
use waylan_ui::CandidateBoxRenderer;
use wayland_client::globals::registry_queue_init;
use wayland_client::Connection;

use self::globals::{bind_session_v1_globals, bind_session_v2_globals};
use self::poll::run_session_loop_step;
use crate::error::WaylandRunnerError;
use crate::session_common::{
    apply_session_setup, SessionSetupOptions, SessionSetupTargets, WaylandSessionLifecycle,
};
use crate::session_v1::{WaylanV1Session, WaylandV1SurfaceContext, WaylandV1WatcherContext};
use crate::session_v2::WaylanV2Session;

/// 启动并运行 Wayland V1 输入法事件循环（使用已初始化的会话实例）
///
/// # 设计原理
/// - **实现初衷**：为早期 Wayland 合成器（如 wlroots 早期版本、支持 `zwp_input_method_v1` 的桌面环境）
///   提供完整兼容器，通过 InputPanel 或 LayerShell 悬浮渲染候选框。
/// - **核心优势**：兼容性广泛，支持基于 XKB 的按键直接抓取与上下文内联提交流程。
/// - **代价与局限**：协议缺乏焦点光标精准矩形广播，需依赖光标防溢出计算估算坐标。
///
/// # Errors
/// 当连接 Wayland 合成器失败、缺少必要全局协议或事件队列派发异常时返回对应的细粒度错误。
pub fn run_session_v1_loop(mut session: WaylanV1Session) -> Result<(), WaylandRunnerError> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    bind_session_v1_globals(&globals, &qh, &mut session)?;

    loop {
        if !run_session_loop_step(&mut event_queue, &mut session)? {
            info!("[微澜 Wayland] 收到退出请求信号，正在安全退出 V1 事件循环。");
            return Ok(());
        }
    }
}

/// 启动并运行现代 Wayland V2 输入法事件循环（若协议不可用或发生错误，返回错误同时移交 session 实例以便降级）
///
/// # 设计原理
/// - **实现初衷**：在宿主环境不支持 V2 协议或初始化失败时，避免销毁已加载的词库算法引擎与配置状态，支持无缝降级。
/// - **核心优势**：通过所有权移交返回当前会话实例，调用方可重用算法状态直接拉起 V1 或其他备用后端，消除二次初始化耗时。
/// - **代价与局限**：函数签名返回携带 `Box<WaylanV2Session>` 的复合元组错误，相较于纯错误类型契约略为复杂。
///
/// # Errors
/// 当连接 Wayland 合成器失败、缺少必需全局协议或事件队列异常时返回错误及当前会话实例。
pub fn run_session_v2_loop_fallible(
    mut session: WaylanV2Session,
) -> Result<(), (WaylandRunnerError, Box<WaylanV2Session>)> {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => return Err((e.into(), Box::new(session))),
    };
    let (globals, mut event_queue) = match registry_queue_init(&conn) {
        Ok(res) => res,
        Err(e) => return Err((e.into(), Box::new(session))),
    };
    let qh = event_queue.handle();

    if let Err(e) = bind_session_v2_globals(&globals, &qh, &mut session) {
        return Err((e, Box::new(session)));
    }

    loop {
        match run_session_loop_step(&mut event_queue, &mut session) {
            Ok(true) => continue,
            Ok(false) => {
                info!("[微澜 Wayland] 收到退出请求信号，正在安全退出 V2 事件循环。");
                return Ok(());
            }
            Err(e) => return Err((e, Box::new(session))),
        }
    }
}

/// 启动并运行现代 Wayland V2 输入法事件循环（适用于 KDE Plasma 6 / GNOME / Hyprland / Sway）
///
/// # 设计原理
/// - **实现初衷**：对接现代 Wayland `zwp_input_method_v2` 规范，通过双缓冲事务完成按键拦截、周围文本感知与光标矩形对齐。
/// - **核心优势**：支持虚拟键盘事件回退机制，按键不拦截时交由席位原生转发，且支持客户端按键长按连击。
/// - **代价与局限**：对合成器协议栈要求严格，必须同时存在 `zwp_input_method_manager_v2` 与 `zwp_virtual_keyboard_v1`。
///
/// # Errors
/// 当连接 Wayland 合成器失败、缺少必需全局协议或事件队列异常时返回对应的细粒度错误。
pub fn run_session_v2_loop(session: WaylanV2Session) -> Result<(), WaylandRunnerError> {
    run_session_v2_loop_fallible(session).map_err(|(e, _)| e)
}

/// 执行通用的 Wayland 会话运行配置与词库热重载
pub fn reload_wayland_session<S: WaylandSessionLifecycle>(session: &mut S) {
    let cfg = session.runtime().reload_from_disk();
    session
        .renderer_mut()
        .apply_ui_config(&cfg.ui, cfg.ui.theme);
    if !session.runtime().core.engine.raw_input().is_empty() {
        session.refresh_surface();
    }
}

/// Wayland 统一会话参数配置包
pub struct WaylandSessionOptions<'a> {
    pub engine: AlgoEngine,
    pub renderer: CandidateBoxRenderer,
    pub cfg: &'a AppConfig,
    pub prefer_v2: bool,
    pub watched_paths: Vec<PathBuf>,
    pub initial_mtime: Option<FileSignature>,
    pub shared_flags: Option<SharedInputFlags>,
    pub mode_notify_tx: Option<SyncSender<()>>,
    pub reload_requested: Option<Arc<AtomicBool>>,
    pub exit_requested: Option<Arc<AtomicBool>>,
    pub loop_wakeup: Option<LoopWakeup>,
    pub voice_provider: Option<Box<dyn VoiceProvider>>,
}

/// 复用 V2 会话失败后回收的运行时状态降级拉起 V1 会话
fn run_v1_fallback_session(
    returned_session: Box<WaylanV2Session>,
    exit_requested: Option<Arc<AtomicBool>>,
    loop_wakeup: Option<LoopWakeup>,
) -> Result<(), WaylandRunnerError> {
    let session_v1 = WaylanV1Session {
        runtime: returned_session.runtime,
        surface: WaylandV1SurfaceContext {
            surface_manager: returned_session.surface.surface_manager,
            ..Default::default()
        },
        watcher: WaylandV1WatcherContext {
            watched_paths: returned_session.watcher.watched_paths,
            last_watched_mtime: returned_session.watcher.last_watched_mtime,
            exit_requested,
            loop_wakeup,
            reload_callback: Some(Box::new(reload_wayland_session)),
            ..Default::default()
        },
        ..WaylanV1Session::default()
    };
    run_session_v1_loop(session_v1)
}

/// 启动 Wayland V2 会话并在协议不可用或运行异常时优雅降级至 V1 会话
fn run_v2_session(
    engine: AlgoEngine,
    setup_opts: SessionSetupOptions<'_>,
    exit_requested: Option<Arc<AtomicBool>>,
    loop_wakeup: Option<LoopWakeup>,
) -> Result<(), WaylandRunnerError> {
    info!("[微澜 Wayland] 启用现代 Wayland V2 协议会话。");
    let mut session_v2 = WaylanV2Session::new(engine);
    apply_session_setup(
        SessionSetupTargets {
            runtime: &mut session_v2.runtime,
            renderer: &mut session_v2.surface.surface_manager.renderer,
            watcher: &mut session_v2.watcher,
            reload_cb: reload_wayland_session,
        },
        setup_opts,
    );

    match run_session_v2_loop_fallible(session_v2) {
        Ok(()) => Ok(()),
        Err((e, returned_session)) => {
            if exit_requested
                .as_ref()
                .is_some_and(|f| f.load(Ordering::Acquire))
            {
                info!("[微澜 Wayland] 收到退出请求信号，V2 会话安全退出。");
                return Ok(());
            }
            warn!(
                "[微澜 Wayland 警告] Wayland V2 会话建立或运行失败 ({})，正在优雅降级至广泛兼容的 V1 会话...",
                e
            );
            run_v1_fallback_session(returned_session, exit_requested, loop_wakeup)
        }
    }
}

/// 启动 Wayland V1 兼容会话
fn run_v1_session(
    engine: AlgoEngine,
    setup_opts: SessionSetupOptions<'_>,
) -> Result<(), WaylandRunnerError> {
    info!("[微澜 Wayland] 启用兼容 Wayland V1 协议会话。");
    let mut session_v1 = WaylanV1Session::with_engine(engine);
    apply_session_setup(
        SessionSetupTargets {
            runtime: &mut session_v1.runtime,
            renderer: &mut session_v1.surface.surface_manager.renderer,
            watcher: &mut session_v1.watcher,
            reload_cb: reload_wayland_session,
        },
        setup_opts,
    );
    run_session_v1_loop(session_v1)
}

/// 统一启动并运行 Wayland 输入法事件循环（支持 V2 优先并在必要时自动优雅降级至 V1）
///
/// # Errors
/// 当合成器无法连接或底层协议严重异常且无法降级时返回错误。
pub fn run_wayland_session(opts: WaylandSessionOptions<'_>) -> Result<(), WaylandRunnerError> {
    let prefer_v2 = opts.prefer_v2;
    let engine = opts.engine;
    let exit_requested = opts.exit_requested.clone();
    let loop_wakeup = opts.loop_wakeup.clone();
    let setup_opts = SessionSetupOptions {
        renderer: opts.renderer,
        cfg: opts.cfg,
        watched_paths: opts.watched_paths,
        initial_mtime: opts.initial_mtime,
        shared_flags: opts.shared_flags,
        mode_notify_tx: opts.mode_notify_tx,
        reload_requested: opts.reload_requested,
        exit_requested: opts.exit_requested.clone(),
        loop_wakeup: opts.loop_wakeup.clone(),
        voice_provider: opts.voice_provider,
    };

    if prefer_v2 {
        run_v2_session(engine, setup_opts, exit_requested, loop_wakeup)
    } else {
        run_v1_session(engine, setup_opts)
    }
}
