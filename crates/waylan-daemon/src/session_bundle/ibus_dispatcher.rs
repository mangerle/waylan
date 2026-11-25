//! IBus 协议引擎会话派发与宿主适配器
//!
//! # 设计原理
//! - **实现初衷**：将守护进程与 `waylan-ibus` 适配库之间的配置中继、菜单动作处理与优雅退场整合为单一模块。
//! - **核心优势**：消除 `main.rs` 中的冗长适配样板代码，实现协议派发与主入口生命周期的明确解耦。
//! - **代价与局限**：宿主配置变更需通过适配器回调映射至 `IbusSessionCore`。

use crate::error::DaemonError;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use log::info;
use waylan_algo::AlgoEngine;
use waylan_core::config::AppConfig;
use waylan_ibus::keys::IbusSessionCore;
use waylan_ibus::{
    IbusConfigReloader, IbusMenuActionHandler, IbusSessionOptions, IbusShutdownCoordinator,
    run_ibus_session,
};

use super::{HostReloadBridge, HostReloadBridgeOptions, build_voice_provider};
use crate::desktop::sync_gnome_extension_state_async;
use crate::system::{ShutdownCoordinator, SubprocessManager};
use crate::tray::TrayService;

/// IBus 会话派发参数包
pub struct IbusDispatchOptions<'a> {
    pub engine: AlgoEngine,
    pub cfg: &'a AppConfig,
    pub is_ibus_arg: bool,
    pub watched_paths: Vec<PathBuf>,
    pub initial_mtime: Option<waylan_sys::FileSignature>,
    pub tray: Option<&'a TrayService>,
    pub subprocess_manager: Arc<SubprocessManager>,
    pub settings_running: Arc<AtomicBool>,
    pub coordinator: ShutdownCoordinator,
}

/// 宿主 IBus 菜单项外部动作处理器
struct DaemonIbusMenuHandler {
    subprocess_manager: Arc<SubprocessManager>,
    settings_running: Arc<AtomicBool>,
    coordinator: ShutdownCoordinator,
}

impl IbusMenuActionHandler for DaemonIbusMenuHandler {
    fn open_settings(&self, _unused_flag: &Arc<AtomicBool>) {
        // 统一使用会话唯一的 settings_running 与 SubprocessManager，消除托盘菜单与 IBus 菜单双头重入风险
        self.subprocess_manager
            .launch_settings(&self.settings_running);
    }

    fn view_log(&self) {
        self.subprocess_manager.launch_view_log();
    }

    fn restart(&self) {
        info!(
            "[微澜守护进程] 收到 IBus 原生菜单重启指令，正在排队等待旧实例完全释放后拉起新实例..."
        );
        self.coordinator.trigger_graceful_restart();
    }

    fn quit(&self) {
        info!("[微澜守护进程] 收到 IBus 原生菜单退出指令，正在触发优雅停机与数据落盘流程...");
        self.coordinator.request_shutdown();
    }
}

/// 宿主 IBus 配置热重载与 GNOME 桌面集成变迁驱动器
struct DaemonIbusConfigReloader {
    bridge: HostReloadBridge,
}

impl IbusConfigReloader for DaemonIbusConfigReloader {
    fn reload(&self, core: &mut IbusSessionCore) -> bool {
        let new_cfg = AppConfig::load_or_create();
        if self.bridge.handle_reload(&new_cfg, false, false) {
            return true;
        }
        core.runtime_mut().reload_config(&new_cfg);
        false
    }

    fn post_reload(&self) {
        self.bridge.flush_pending_desktop_sync();
    }
}

/// 构建 IBus 会话核心实例，并按需挂载托盘状态联动与初始输入配置
fn build_ibus_session_core(
    engine: AlgoEngine,
    tray: Option<&TrayService>,
    initial_cfg: &AppConfig,
) -> IbusSessionCore {
    let mut session_core = IbusSessionCore::new(engine);
    if let Some(voice) = build_voice_provider(initial_cfg) {
        session_core = session_core.with_voice_provider(voice);
    }
    if let Some(tray) = tray {
        session_core.attach_tray(tray.shared_flags().clone(), Some(tray.icon_notifier()));
    }
    session_core.runtime_mut().apply_initial_config(initial_cfg);
    session_core
}

/// 构建 IBus 会话停机协调器，串联宿主停机标志、事件循环唤醒与优雅重启回调
fn build_ibus_coordinator(coordinator: &ShutdownCoordinator) -> IbusShutdownCoordinator {
    IbusShutdownCoordinator::new(
        coordinator.flag(),
        coordinator.loop_wakeup(),
        Some(Arc::new({
            let coord = coordinator.clone();
            move || coord.trigger_graceful_restart()
        })),
    )
}

/// 运行 IBus 会话并统一收敛异常退出与停机退出码
pub fn dispatch_ibus_session(opts: IbusDispatchOptions<'_>) -> Result<(), DaemonError> {
    // 仅当托盘服务真实完成 D-Bus 会话注册时才隐藏 IBus 原生菜单，避免 DBus 连不上时两头无菜单
    let tray_ready = opts.tray.is_some_and(TrayService::is_dbus_ready);
    info!(
        "[微澜守护进程] 启用 IBus / D-Bus 输入法引擎会话 (展示形态: {}, 原生指示器属性上报就绪)。",
        if tray_ready {
            "形态 B 专属彩色托盘"
        } else {
            "形态 A 原生顶栏"
        }
    );

    let reload_bridge = HostReloadBridge::new(HostReloadBridgeOptions {
        cfg: opts.cfg,
        has_tray: tray_ready,
        use_ibus_backend: true,
        coordinator: opts.coordinator.clone(),
        tray_sync: opts.tray.map(TrayService::sync_handle),
    });
    // 启动即后台对齐一次扩展加载状态：扩展可能因系统升级或用户在扩展管理器中手动关闭而脱节
    sync_gnome_extension_state_async(reload_bridge.current_intent());
    let session_core = build_ibus_session_core(opts.engine, opts.tray, opts.cfg);

    let menu_handler = Arc::new(DaemonIbusMenuHandler {
        subprocess_manager: opts.subprocess_manager,
        settings_running: opts.settings_running,
        coordinator: opts.coordinator.clone(),
    });
    let config_reloader = Arc::new(DaemonIbusConfigReloader {
        bridge: reload_bridge,
    });
    let ibus_coordinator = build_ibus_coordinator(&opts.coordinator);

    let ibus_res = run_ibus_session(IbusSessionOptions {
        session_core,
        is_ibus_child_mode: opts.is_ibus_arg,
        watched_paths: opts.watched_paths,
        initial_mtime: opts.initial_mtime,
        reload_requested: opts.tray.as_ref().map(|t| t.reload_flag()),
        coordinator: ibus_coordinator,
        suppress_properties: tray_ready,
        menu_handler: Some(menu_handler),
        config_reloader: Some(config_reloader),
    });
    super::finalize_session_loop_result(ibus_res, &opts.coordinator, "IBus")
}
