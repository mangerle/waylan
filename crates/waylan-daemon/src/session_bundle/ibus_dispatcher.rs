//! IBus 协议引擎会话派发与宿主适配器
//!
//! # 设计原理
//! - **实现初衷**：将守护进程与 `waylan-ibus` 适配库之间的配置中继、菜单动作处理与优雅退场整合为单一模块。
//! - **核心优势**：消除 `main.rs` 中的冗长适配样板代码，实现协议派发与主入口生命周期的明确解耦。
//! - **代价与局限**：宿主配置变更需通过适配器回调映射至 `IbusSessionCore`。

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use log::info;
use waylan_algo::AlgoEngine;
use waylan_core::config::AppConfig;
use waylan_ibus::keys::IbusSessionCore;
use waylan_ibus::{
    run_ibus_session, IbusConfigReloader, IbusMenuActionHandler, IbusSessionOptions,
    IbusShutdownCoordinator,
};

use super::config_bundle::SessionConfigBundle;
use crate::system::{ShutdownCoordinator, SubprocessManager};
use crate::tray::TrayService;

/// IBus 会话派发参数包
pub struct IbusDispatchOptions<'a> {
    pub engine: AlgoEngine,
    pub is_ibus_arg: bool,
    pub should_spawn_tray: bool,
    pub exclusive_source: bool,
    pub watched_paths: Vec<PathBuf>,
    pub initial_mtime: Option<SystemTime>,
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
}

impl IbusConfigReloader for DaemonIbusConfigReloader {
    fn reload(&self, core: &mut IbusSessionCore) -> bool {
        let bundle = SessionConfigBundle::load_current();
        let hide_ext = bundle.exclusive_input_source;
        let current_has_tray = core.shared_is_chinese.is_some();
        let new_cfg = AppConfig::load_or_create();
        let is_gnome = env::var("XDG_CURRENT_DESKTOP")
            .map(|d| d.to_uppercase().contains("GNOME"))
            .unwrap_or(false);
        let expected_tray = new_cfg.enable_tray.unwrap_or(!is_gnome);

        if expected_tray != current_has_tray {
            return true;
        }

        apply_bundle_to_ibus_core(bundle, core);
        let old_exclusive = self.exclusive_input_source.swap(hide_ext, Ordering::AcqRel);
        if hide_ext != old_exclusive {
            waylan_ibus::desktop_integration::ensure_gnome_input_source_registered(hide_ext);
            waylan_ibus::desktop_integration::sync_gnome_hide_indicator_extension(hide_ext);
        }
        false
    }
}

/// 将统一配置包映射并应用到 IBus 核心会话中
fn apply_bundle_to_ibus_core(bundle: SessionConfigBundle, core: &mut IbusSessionCore) {
    core.candidate_index_style = bundle.candidate_index_style;
    core.candidate_orientation = bundle.candidate_orientation;
    core.core.traditional_shortcut = bundle.traditional_shortcut;
    core.core.full_width_char_shortcut = bundle.full_width_char_shortcut;
    core.core.full_width_punct_shortcut = bundle.full_width_punct_shortcut;
    core.voice_manager.update_config(bundle.voice.clone());
    bundle.apply_to_core(&mut core.core);
}

/// 运行 IBus 会话并统一收敛异常退出与停机退出码
pub fn dispatch_ibus_session(opts: IbusDispatchOptions<'_>) -> Result<(), Box<dyn Error>> {
    info!(
        "[微澜守护进程] 启用 IBus / D-Bus 输入法引擎会话 (展示形态: {}, 原生指示器属性上报就绪)。",
        if opts.should_spawn_tray {
            "形态 B 专属彩色托盘"
        } else {
            "形态 A 原生顶栏"
        }
    );

    let mut session_core = IbusSessionCore::new(opts.engine);
    session_core.shared_is_chinese = opts.tray.as_ref().map(|t| t.mode_flag());
    session_core.shared_is_caps_locked = opts.tray.as_ref().map(|t| t.caps_lock_flag());
    session_core.shared_traditional_chinese =
        opts.tray.as_ref().map(|t| t.traditional_chinese_flag());
    session_core.shared_full_width_punct = opts.tray.as_ref().map(|t| t.full_width_punct_flag());
    session_core.shared_full_width_char = opts.tray.as_ref().map(|t| t.full_width_char_flag());
    session_core.mode_notify_tx = opts.tray.as_ref().map(|t| t.icon_notifier());
    let initial_bundle = SessionConfigBundle::load_current();
    apply_bundle_to_ibus_core(initial_bundle, &mut session_core);

    let menu_handler = Arc::new(DaemonIbusMenuHandler {
        subprocess_manager: Arc::new(SubprocessManager::new(opts.coordinator.flag())),
    });
    let config_reloader = Arc::new(DaemonIbusConfigReloader {
        exclusive_input_source: AtomicBool::new(opts.exclusive_source),
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
        Err(e) => {
            log::error!("[微澜守护进程 错误] IBus 输入法会话异常退出: {}", e);
            process::exit(1);
        }
    }
}
