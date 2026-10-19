//! Wayland 协议引擎会话派发器
//!
//! # 设计原理
//! - **实现初衷**：将上层守护进程的托盘通知、配置热载与退出协调信号映射并派发至 `waylan-wayland` 统一驱动中枢。
//! - **核心优势**：消除冗余的 V1/V2 降级胶水代码与自创 trait，全权委托 `waylan_wayland::run_wayland_session` 统一纳管。
//! - **代价与局限**：协议级降级细节完全内聚于 `waylan-wayland` 内部。

use crate::error::DaemonError;
use std::path::PathBuf;
use std::process;

use log::info;
use waylan_algo::AlgoEngine;
use waylan_core::config::AppConfig;
use waylan_ui::CandidateBoxRenderer;
use waylan_wayland::{run_wayland_session, WaylandCapabilities, WaylandSessionOptions};

use crate::system::ShutdownCoordinator;
use crate::tray::TrayService;

/// Wayland 会话派发参数包
pub struct WaylandDispatchOptions<'a> {
    pub engine: AlgoEngine,
    pub renderer: CandidateBoxRenderer,
    pub cfg: &'a AppConfig,
    pub caps: &'a WaylandCapabilities,
    pub watched_paths: Vec<PathBuf>,
    pub initial_mtime: Option<waylan_sys::FileSignature>,
    pub tray: Option<&'a TrayService>,
    pub coordinator: &'a ShutdownCoordinator,
}

/// 运行 Wayland 会话并处理异常退出
pub fn dispatch_wayland_session(opts: WaylandDispatchOptions<'_>) -> Result<(), DaemonError> {
    let prefer_v2 = opts.caps.has_input_method_v2 && opts.caps.has_seat;
    let voice_provider = Some(Box::new(waylan_voice::VoiceInteractionManager::new(
        opts.cfg.voice.clone(),
    )) as Box<dyn waylan_core::input::VoiceProvider>);

    let loop_result = run_wayland_session(WaylandSessionOptions {
        engine: opts.engine,
        renderer: opts.renderer,
        cfg: opts.cfg,
        prefer_v2,
        watched_paths: opts.watched_paths,
        initial_mtime: opts.initial_mtime,
        shared_flags: opts.tray.as_ref().map(|t| t.shared_flags().clone()),
        mode_notify_tx: opts.tray.as_ref().map(|t| t.icon_notifier()),
        reload_requested: opts.tray.as_ref().map(|t| t.reload_flag()),
        exit_requested: Some(opts.coordinator.flag()),
        loop_wakeup: opts.coordinator.loop_wakeup(),
        voice_provider,
    });

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
