//! IBus 协议引擎会话派发与宿主适配器
//!
//! # 设计原理
//! - **实现初衷**：将守护进程与 `waylan-ibus` 适配库之间的配置中继、菜单动作处理与优雅退场整合为单一模块。
//! - **核心优势**：消除 `main.rs` 中的冗长适配样板代码，实现协议派发与主入口生命周期的明确解耦。
//! - **代价与局限**：宿主配置变更需通过适配器回调映射至 `IbusSessionCore`。

use crate::error::DaemonError;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use log::info;
use waylan_algo::AlgoEngine;
use waylan_core::config::AppConfig;
use waylan_ibus::keys::IbusSessionCore;
use waylan_ibus::{
    run_ibus_session, IbusConfigReloader, IbusMenuActionHandler, IbusSessionOptions,
    IbusShutdownCoordinator,
};

use crate::system::{ShutdownCoordinator, SubprocessManager};
use crate::tray::TrayService;

/// IBus 会话派发参数包
pub struct IbusDispatchOptions<'a> {
    pub engine: AlgoEngine,
    pub is_ibus_arg: bool,
    pub should_spawn_tray: bool,
    pub exclusive_source: bool,
    pub watched_paths: Vec<PathBuf>,
    pub initial_mtime: Option<waylan_sys::FileSignature>,
    pub tray: Option<&'a TrayService>,
    pub coordinator: ShutdownCoordinator,
}

/// 宿主 IBus 菜单项外部动作处理器
struct DaemonIbusMenuHandler {
    subprocess_manager: Arc<SubprocessManager>,
}

impl IbusMenuActionHandler for DaemonIbusMenuHandler {
    fn open_settings(&self, settings_running: &Arc<AtomicBool>) {
        self.subprocess_manager.launch_settings(settings_running);
    }

    fn view_log(&self) {
        self.subprocess_manager.launch_view_log();
    }
}

/// 宿主 IBus 配置热重载与指示器形态变迁驱动器
struct DaemonIbusConfigReloader {
    exclusive_input_source: AtomicBool,
    pending_sync_exclusive: AtomicBool,
}

impl IbusConfigReloader for DaemonIbusConfigReloader {
    fn reload(&self, core: &mut IbusSessionCore) -> bool {
        let new_cfg = AppConfig::load_or_create();
        let hide_ext = new_cfg.desktop.exclusive_input_source;
        let current_has_tray = core.has_tray();
        let is_gnome = env::var("XDG_CURRENT_DESKTOP")
            .map(|d| d.to_uppercase().contains("GNOME"))
            .unwrap_or(false);
        let expected_tray = match new_cfg.desktop.enable_tray {
            waylan_core::TrayDisplayMode::Always => true,
            waylan_core::TrayDisplayMode::Never => false,
            waylan_core::TrayDisplayMode::Auto => !is_gnome,
        };

        if expected_tray != current_has_tray {
            return true;
        }

        apply_config_to_ibus_core(&new_cfg, core);
        let old_exclusive = self.exclusive_input_source.swap(hide_ext, Ordering::AcqRel);
        if hide_ext != old_exclusive {
            // 标记待在锁守卫释放后的 post_reload 中安全执行桌面集成外部命令
            self.pending_sync_exclusive.store(true, Ordering::Release);
        }
        false
    }

    fn post_reload(&self) {
        if self.pending_sync_exclusive.swap(false, Ordering::AcqRel) {
            let hide_ext = self.exclusive_input_source.load(Ordering::Acquire);
            crate::desktop::ensure_gnome_input_source_registered(hide_ext);
            crate::desktop::sync_gnome_hide_indicator_extension(hide_ext);
        }
    }
}

/// 将统一配置包映射并应用到 IBus 核心会话中
fn apply_config_to_ibus_core(cfg: &AppConfig, core: &mut IbusSessionCore) {
    core.set_candidate_index_style(cfg.ui.candidate_index_style);
    core.set_candidate_orientation(cfg.ui.candidate_orientation);
    core.runtime_mut().reload_config(cfg);
}

/// 应用 IBus 会话启动阶段的初始配置（仅调用一次，负责初始化中英文模式）
fn apply_initial_config_to_ibus_core(cfg: &AppConfig, core: &mut IbusSessionCore) {
    core.set_candidate_index_style(cfg.ui.candidate_index_style);
    core.set_candidate_orientation(cfg.ui.candidate_orientation);
    core.runtime_mut().apply_initial_config(cfg);
}

/// 运行 IBus 会话并统一收敛异常退出与停机退出码
pub fn dispatch_ibus_session(opts: IbusDispatchOptions<'_>) -> Result<(), DaemonError> {
    info!(
        "[微澜守护进程] 启用 IBus / D-Bus 输入法引擎会话 (展示形态: {}, 原生指示器属性上报就绪)。",
        if opts.should_spawn_tray {
            "形态 B 专属彩色托盘"
        } else {
            "形态 A 原生顶栏"
        }
    );

    let initial_cfg = AppConfig::load_or_create();
    let mut session_core = IbusSessionCore::new(opts.engine).with_voice_provider(Box::new(
        waylan_voice::VoiceInteractionManager::new(initial_cfg.voice.clone()),
    ));
    if let Some(tray) = opts.tray {
        session_core.attach_tray(
            tray.shared_flags().clone(),
            opts.tray.as_ref().map(|t| t.icon_notifier()),
        );
    }
    apply_initial_config_to_ibus_core(&initial_cfg, &mut session_core);

    let menu_handler = Arc::new(DaemonIbusMenuHandler {
        subprocess_manager: Arc::new(SubprocessManager::new(opts.coordinator.flag())),
    });
    let config_reloader = Arc::new(DaemonIbusConfigReloader {
        exclusive_input_source: AtomicBool::new(opts.exclusive_source),
        pending_sync_exclusive: AtomicBool::new(false),
    });
    let ibus_coordinator = IbusShutdownCoordinator::new(
        opts.coordinator.flag(),
        opts.coordinator.loop_wakeup(),
        Some(Arc::new({
            let coord = opts.coordinator.clone();
            move || coord.trigger_graceful_restart()
        })),
    );

    let ibus_res = run_ibus_session(IbusSessionOptions {
        session_core,
        is_ibus_child_mode: opts.is_ibus_arg,
        watched_paths: opts.watched_paths,
        initial_mtime: opts.initial_mtime,
        reload_requested: opts.tray.as_ref().map(|t| t.reload_flag()),
        coordinator: ibus_coordinator,
        suppress_properties: false,
        exclusive_input_source: opts.exclusive_source,
        menu_handler: Some(menu_handler),
        config_reloader: Some(config_reloader),
    });
    match ibus_res {
        Ok(_) => {
            info!("[微澜守护进程] IBus 输入法事件循环已正常退出，所有资源已安全释放。");
            Ok(())
        }
        Err(_) if opts.coordinator.is_shutting_down() => {
            info!("[微澜守护进程] 收到退出信号，IBus 服务安全退场。");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
