//! Wayland V1/V2 协议引擎会话装配与异常降级派发器
//!
//! # 设计原理
//! - **实现初衷**：统一收敛 Wayland 原生协议会话装配、运行时配置注入、V2 故障自动降级至 V1，以及退出码处理。
//! - **核心优势**：消除 `main.rs` 中大段的 Wayland 初始化与协议协商匹配代码，实现纯粹的关注点分离。
//! - **代价与局限**：调用方需提供已初始化的渲染器与底层合成器能力探测快照。

use crate::error::DaemonError;
use std::path::PathBuf;
use std::process;

use log::{info, warn};
use waylan_algo::AlgoEngine;
use waylan_core::config::{AppConfig, ThemeMode};
use waylan_ui::{CandidateBoxRenderer, UiTheme};
use waylan_wayland::session_v1::WaylanV1Session;
use waylan_wayland::session_v2::WaylanV2Session;
use waylan_wayland::{run_session_v1_loop, run_session_v2_loop_fallible, WaylandCapabilities};

use crate::system::ShutdownCoordinator;
use crate::tray::TrayService;

/// 抽象具备热重载能力的 Wayland 会话接口
pub trait ReloadableWaylandSession {
    /// 获取候选框渲染器的可变引用
    fn renderer_mut(&mut self) -> &mut CandidateBoxRenderer;
    /// 获取输入法运行时的可变引用
    fn runtime_mut(&mut self) -> &mut waylan_engine::ImeRuntime;
    /// 触发刷新候选框悬浮表面
    fn refresh_surface(&mut self);
}

impl ReloadableWaylandSession for WaylanV1Session {
    #[inline]
    fn renderer_mut(&mut self) -> &mut CandidateBoxRenderer {
        &mut self.surface_manager.renderer
    }

    #[inline]
    fn runtime_mut(&mut self) -> &mut waylan_engine::ImeRuntime {
        &mut self.runtime
    }

    #[inline]
    fn refresh_surface(&mut self) {
        self.refresh_candidate_surface();
    }
}

impl ReloadableWaylandSession for WaylanV2Session {
    #[inline]
    fn renderer_mut(&mut self) -> &mut CandidateBoxRenderer {
        &mut self.surface_manager.renderer
    }

    #[inline]
    fn runtime_mut(&mut self) -> &mut waylan_engine::ImeRuntime {
        &mut self.runtime
    }

    #[inline]
    fn refresh_surface(&mut self) {
        self.refresh_candidate_surface();
    }
}

/// 执行通用的 Wayland 会话运行配置与词库热重载
pub fn reload_wayland_session<S: ReloadableWaylandSession>(session: &mut S) {
    let cfg = AppConfig::load_or_create();
    let theme = match cfg.ui.theme {
        ThemeMode::Light => UiTheme::light(),
        ThemeMode::Dark => UiTheme::dark(),
    };
    session.renderer_mut().theme = theme;
    session.renderer_mut().font_size = cfg.ui.font_size;
    session.runtime_mut().reload_config(&cfg);
    if !session.runtime_mut().core.engine.raw_input().is_empty() {
        session.refresh_surface();
    }
}

/// 执行 Wayland V1 会话的运行配置与词库热重载
pub fn reload_wayland_v1_session(session: &mut WaylanV1Session) {
    reload_wayland_session(session);
}

/// 执行 Wayland V2 会话的运行配置与词库热重载
pub fn reload_wayland_v2_session(session: &mut WaylanV2Session) {
    reload_wayland_session(session);
}

/// Wayland 内部会话参数包
struct WaylandSessionParams<'a> {
    engine: AlgoEngine,
    renderer: CandidateBoxRenderer,
    cfg: &'a AppConfig,
    watched_paths: Vec<PathBuf>,
    initial_mtime: Option<waylan_core::FileSignature>,
    tray: Option<&'a TrayService>,
    coordinator: &'a ShutdownCoordinator,
}

/// 运行 Wayland V1 会话
fn run_v1_session(params: WaylandSessionParams<'_>) -> Result<(), DaemonError> {
    let mut session = WaylanV1Session::with_engine(params.engine);
    session.surface_manager.renderer = params.renderer;
    session.watched_paths = params.watched_paths;
    session.last_watched_mtime = params.initial_mtime;
    session.runtime.shared_flags = params.tray.as_ref().map(|t| t.shared_flags().clone());
    session.runtime.mode_notify_tx = params.tray.as_ref().map(|t| t.icon_notifier());
    session.runtime.reload_requested = params.tray.as_ref().map(|t| t.reload_flag());
    session.exit_requested = Some(params.coordinator.flag());
    session.loop_wakeup = params.coordinator.loop_wakeup();
    session.reload_callback = Some(Box::new(reload_wayland_v1_session));
    session.runtime.reload_config(params.cfg);
    run_session_v1_loop(session).map_err(Into::into)
}

/// 运行 Wayland V2 会话，并在协议失败时自动降级至 V1 会话
fn run_v2_session_with_v1_fallback(params: WaylandSessionParams<'_>) -> Result<(), DaemonError> {
    info!("[微澜守护进程] 检测到 zwp_input_method_manager_v2 支持，启用现代 Wayland V2 协议会话。");
    let mut session_v2 = WaylanV2Session::new(params.engine);
    session_v2.surface_manager.renderer = params.renderer;
    session_v2.watched_paths = params.watched_paths;
    session_v2.last_watched_mtime = params.initial_mtime;
    session_v2.runtime.shared_flags = params.tray.as_ref().map(|t| t.shared_flags().clone());
    session_v2.runtime.mode_notify_tx = params.tray.as_ref().map(|t| t.icon_notifier());
    session_v2.runtime.reload_requested = params.tray.as_ref().map(|t| t.reload_flag());
    session_v2.exit_requested = Some(params.coordinator.flag());
    session_v2.loop_wakeup = params.coordinator.loop_wakeup();
    session_v2.reload_callback = Some(Box::new(reload_wayland_v2_session));
    session_v2.runtime.reload_config(params.cfg);
    let v2_result = run_session_v2_loop_fallible(session_v2);
    match v2_result {
        Ok(()) => Ok(()),
        Err((e, returned_session)) => {
            if params.coordinator.is_shutting_down() {
                info!("[微澜守护进程] 检测到优雅退出请求，服务安全退场。");
                return Ok(());
            }
            warn!(
                "[微澜守护进程 警告] Wayland V2 会话建立或运行失败 ({})，正在优雅降级至广泛兼容的 V1 会话...",
                e
            );
            let session = WaylanV1Session {
                runtime: returned_session.runtime,
                surface_manager: returned_session.surface_manager,
                watched_paths: returned_session.watched_paths,
                last_watched_mtime: returned_session.last_watched_mtime,
                exit_requested: Some(params.coordinator.flag()),
                loop_wakeup: params.coordinator.loop_wakeup(),
                reload_callback: Some(Box::new(reload_wayland_v1_session)),
                ..WaylanV1Session::default()
            };
            run_session_v1_loop(session).map_err(Into::into)
        }
    }
}

/// Wayland 会话派发参数包
pub struct WaylandDispatchOptions<'a> {
    pub engine: AlgoEngine,
    pub renderer: CandidateBoxRenderer,
    pub cfg: &'a AppConfig,
    pub caps: &'a WaylandCapabilities,
    pub watched_paths: Vec<PathBuf>,
    pub initial_mtime: Option<waylan_core::FileSignature>,
    pub tray: Option<&'a TrayService>,
    pub coordinator: &'a ShutdownCoordinator,
}

/// 运行 Wayland 会话并处理异常退出
pub fn dispatch_wayland_session(opts: WaylandDispatchOptions<'_>) -> Result<(), DaemonError> {
    let session_params = WaylandSessionParams {
        engine: opts.engine,
        renderer: opts.renderer,
        cfg: opts.cfg,
        watched_paths: opts.watched_paths,
        initial_mtime: opts.initial_mtime,
        tray: opts.tray,
        coordinator: opts.coordinator,
    };

    let loop_result = if opts.caps.has_input_method_v2 && opts.caps.has_seat {
        run_v2_session_with_v1_fallback(session_params)
    } else {
        run_v1_session(session_params)
    };

    match loop_result {
        Ok(_) => {
            info!("[微澜守护进程] Wayland 输入法事件循环已正常退出，所有资源已安全释放。");
        }
        Err(e) => {
            if opts.coordinator.is_shutting_down() {
                info!("[微澜守护进程] 收到退出信号或托盘退出请求，服务安全退场。");
                return Ok(());
            }
            log::error!(
                "[微澜守护进程 错误] Wayland 输入法事件循环异常退出或无法启动: {}，准备终止进程并释放资源。",
                e
            );
            process::exit(1);
        }
    }

    Ok(())
}
