//! IBus 协议引擎会话派发与宿主适配器
//!
//! # 设计原理
//! - **实现初衷**：将守护进程与 `waylan-ibus` 适配库之间的配置中继、菜单动作处理与优雅退场整合为单一模块。
//! - **核心优势**：消除 `main.rs` 中的冗长适配样板代码，实现协议派发与主入口生命周期的明确解耦。
//! - **代价与局限**：宿主配置变更需通过适配器回调映射至 `IbusSessionCore`。

use crate::error::DaemonError;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use log::info;
use waylan_algo::AlgoEngine;
use waylan_core::config::AppConfig;
use waylan_ibus::keys::IbusSessionCore;
use waylan_ibus::{
    IbusConfigReloader, IbusMenuActionHandler, IbusSessionOptions, IbusShutdownCoordinator,
    run_ibus_session,
};

use crate::desktop::{
    GnomeExtensionIntent, gnome_extension_intent, sync_gnome_extension_state_async,
};
use crate::system::{ShutdownCoordinator, SubprocessManager};
use crate::tray::TrayService;

/// IBus 会话派发参数包
pub struct IbusDispatchOptions<'a> {
    pub engine: AlgoEngine,
    pub cfg: &'a AppConfig,
    pub is_ibus_arg: bool,
    pub should_spawn_tray: bool,
    pub watched_paths: Vec<PathBuf>,
    pub initial_mtime: Option<waylan_sys::FileSignature>,
    pub tray: Option<&'a TrayService>,
    pub coordinator: ShutdownCoordinator,
}

/// 宿主 IBus 菜单项外部动作处理器
struct DaemonIbusMenuHandler {
    subprocess_manager: Arc<SubprocessManager>,
    coordinator: ShutdownCoordinator,
}

impl IbusMenuActionHandler for DaemonIbusMenuHandler {
    fn open_settings(&self, settings_running: &Arc<AtomicBool>) {
        self.subprocess_manager.launch_settings(settings_running);
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
    /// 当前生效的扩展意图：是否隐藏顶栏原生输入源指示器
    hide_indicator: AtomicBool,
    /// 当前生效的扩展意图：是否由扩展渲染微澜自定义候选框
    custom_candidate_view: AtomicBool,
    /// 待锁守卫释放后执行的桌面集成外部命令标记
    pending_sync: AtomicBool,
}

impl DaemonIbusConfigReloader {
    /// 写入最新扩展意图并返回是否发生变迁
    ///
    /// 两项职责必须一起比较：只监听独占输入源会让"候选框模式"的改动静默丢失。
    fn update_intent(&self, intent: GnomeExtensionIntent) -> bool {
        let prev_hide = self
            .hide_indicator
            .swap(intent.hide_indicator, Ordering::AcqRel);
        let prev_view = self
            .custom_candidate_view
            .swap(intent.custom_candidate_view, Ordering::AcqRel);
        prev_hide != intent.hide_indicator || prev_view != intent.custom_candidate_view
    }

    /// 读取当前生效的扩展意图
    fn current_intent(&self) -> GnomeExtensionIntent {
        GnomeExtensionIntent {
            hide_indicator: self.hide_indicator.load(Ordering::Acquire),
            custom_candidate_view: self.custom_candidate_view.load(Ordering::Acquire),
        }
    }
}

impl IbusConfigReloader for DaemonIbusConfigReloader {
    fn reload(&self, core: &mut IbusSessionCore) -> bool {
        let new_cfg = AppConfig::load_or_create();
        let current_has_tray = core.has_tray();
        let is_gnome = waylan_sys::desktop::is_gnome_desktop();
        let expected_tray = match new_cfg.desktop.enable_tray {
            waylan_core::TrayDisplayMode::Always => true,
            waylan_core::TrayDisplayMode::Never => false,
            waylan_core::TrayDisplayMode::Auto => !is_gnome,
        };

        if expected_tray != current_has_tray {
            return true;
        }

        core.runtime_mut().reload_config(&new_cfg);
        if self.update_intent(gnome_extension_intent(&new_cfg)) {
            // 标记待在锁守卫释放后的 post_reload 中安全执行桌面集成外部命令
            self.pending_sync.store(true, Ordering::Release);
        }
        false
    }

    fn post_reload(&self) {
        if self.pending_sync.swap(false, Ordering::AcqRel) {
            let intent = self.current_intent();
            crate::desktop::ensure_gnome_input_source_registered(intent.hide_indicator);
            crate::desktop::sync_gnome_hide_indicator_extension(intent);
        }
    }
}

/// 构建 IBus 会话核心实例，并按需挂载托盘状态联动与初始输入配置
fn build_ibus_session_core(
    engine: AlgoEngine,
    tray: Option<&TrayService>,
    initial_cfg: &AppConfig,
) -> IbusSessionCore {
    let mut session_core = IbusSessionCore::new(engine).with_voice_provider(Box::new(
        waylan_voice::VoiceInteractionManager::new(initial_cfg.voice.clone()),
    ));
    if let Some(tray) = tray {
        session_core.attach_tray(tray.shared_flags().clone(), Some(tray.icon_notifier()));
    }
    session_core.runtime_mut().apply_initial_config(initial_cfg);
    session_core
}

/// 构建宿主侧 IBus 配置热重载驱动器（以启动期配置作为变迁比较基线）
fn build_config_reloader(intent: GnomeExtensionIntent) -> Arc<DaemonIbusConfigReloader> {
    Arc::new(DaemonIbusConfigReloader {
        hide_indicator: AtomicBool::new(intent.hide_indicator),
        custom_candidate_view: AtomicBool::new(intent.custom_candidate_view),
        pending_sync: AtomicBool::new(false),
    })
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
    info!(
        "[微澜守护进程] 启用 IBus / D-Bus 输入法引擎会话 (展示形态: {}, 原生指示器属性上报就绪)。",
        if opts.should_spawn_tray {
            "形态 B 专属彩色托盘"
        } else {
            "形态 A 原生顶栏"
        }
    );

    let initial_intent = gnome_extension_intent(opts.cfg);
    // 启动即后台对齐一次扩展加载状态：扩展可能因系统升级、用户在扩展管理器中手动关闭或首次
    // 安装而与配置脱节，原实现只在配置值变化时才同步，会让脱节状态永久保留。
    sync_gnome_extension_state_async(initial_intent);
    let session_core = build_ibus_session_core(opts.engine, opts.tray, opts.cfg);

    let menu_handler = Arc::new(DaemonIbusMenuHandler {
        subprocess_manager: Arc::new(SubprocessManager::new(opts.coordinator.flag())),
        coordinator: opts.coordinator.clone(),
    });
    let config_reloader = build_config_reloader(initial_intent);
    let ibus_coordinator = build_ibus_coordinator(&opts.coordinator);

    let ibus_res = run_ibus_session(IbusSessionOptions {
        session_core,
        is_ibus_child_mode: opts.is_ibus_arg,
        watched_paths: opts.watched_paths,
        initial_mtime: opts.initial_mtime,
        reload_requested: opts.tray.as_ref().map(|t| t.reload_flag()),
        coordinator: ibus_coordinator,
        // 菜单承载权归"当家的托盘"：专属彩色托盘在跑时由它承载 9 项菜单，抑制引擎属性上报以免重复；
        // 专属托盘未启用时（enable_tray=never 或非 GNOME 宿主）必须发出属性树，否则 IBus /
        // GNOME 原生指示器菜单里只剩系统通用项，微澜功能无从触达。
        suppress_properties: opts.should_spawn_tray,
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
