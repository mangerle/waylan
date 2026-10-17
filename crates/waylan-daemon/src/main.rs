mod cli;
mod error;
mod logger;
mod session_bundle;
mod system;
mod tray;

use cli::{CliCommand, DaemonOptions};
use error::DaemonError;
use log::{info, warn};
use session_bundle::{
    build_algo_engine, check_wayland_capabilities, dispatch_ibus_session, dispatch_wayland_session,
    IbusDispatchOptions, WaylandDispatchOptions,
};
use std::env;
use std::process;
use system::{
    check_another_instance_running, collect_watched_paths, determine_tray_and_exclusive,
    launch_settings_gui, try_acquire_single_instance_lock, ShutdownCoordinator, SingleInstanceLock,
};
pub use waylan_core::config;
use waylan_core::config::{AppConfig, ThemeMode};
use waylan_core::LoopWakeup;
use waylan_ibus::{print_ibus_component_xml, print_ibus_engines_xml};
use waylan_ui::{CandidateBoxRenderer, UiTheme};

fn main() -> Result<(), DaemonError> {
    let args: Vec<String> = env::args().collect();
    let command = match CliCommand::parse(&args) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("错误: {}", e);
            cli::print_main_usage();
            process::exit(2);
        }
    };

    match command {
        CliCommand::Version => {
            println!("waylan {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        CliCommand::Help => {
            cli::print_main_usage();
            Ok(())
        }
        CliCommand::Settings => launch_settings_gui(),
        CliCommand::IbusEnginesXml => {
            print_ibus_engines_xml();
            Ok(())
        }
        CliCommand::IbusComponentXml => {
            print_ibus_component_xml();
            Ok(())
        }
        CliCommand::Dict(dict_cmd) => dict_cmd.execute(),
        CliCommand::Daemon(opts) => run_daemon(opts),
    }
}

/// 启动并运行输入法常驻守护服务
fn run_daemon(opts: DaemonOptions) -> Result<(), DaemonError> {
    if let Err(e) = logger::init_logger() {
        eprintln!("[微澜] 日志系统初始化失败: {}", e);
    }

    let _single_instance_lock = match acquire_daemon_instance_lock(&opts) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => return Ok(()),
        Err(e) => return Err(e),
    };

    let loop_wakeup = LoopWakeup::new().ok();
    let coordinator = ShutdownCoordinator::new(loop_wakeup);
    coordinator.register_system_signals();

    info!("============================================================");
    info!(
        "  微澜 (WayLan) - 现代轻量纯 Rust 中文输入法服务 (版本: {})",
        env!("CARGO_PKG_VERSION")
    );
    info!("============================================================");

    let cfg = AppConfig::load_or_create();
    let Some(mut engine) = build_algo_engine(&cfg, &coordinator)? else {
        return Ok(());
    };

    if opts.self_check {
        cli::run_self_check(&mut engine)?;
        return Ok(());
    }

    if coordinator.is_shutting_down() {
        return Ok(());
    }

    dispatch_active_session(opts, cfg, engine, coordinator)
}

/// 尝试获取单实例运行文件锁并在冲突时优雅处理
fn acquire_daemon_instance_lock(
    opts: &DaemonOptions,
) -> Result<Option<SingleInstanceLock>, DaemonError> {
    match try_acquire_single_instance_lock() {
        Ok(Some(lock)) => Ok(Some(lock)),
        Ok(None) => {
            handle_instance_conflict(opts);
            Ok(None)
        }
        Err(e) => {
            warn!(
                "[微澜] 获取单例运行文件锁遇到异常 ({})，回退至进程扫描校验...",
                e
            );
            if !opts.ibus_backend && !opts.self_check && check_another_instance_running() {
                info!("[微澜] 输入法守护进程已在后台常驻运行，正在为您唤起偏好设置中心...");
                let _ = launch_settings_gui();
                return Ok(None);
            }
            Ok(None)
        }
    }
}

/// 处理单实例冲突唤起逻辑
fn handle_instance_conflict(opts: &DaemonOptions) {
    if !opts.ibus_backend && !opts.self_check {
        info!("[微澜] 输入法守护进程已在后台常驻运行，正在为您唤起偏好设置中心...");
        let _ = launch_settings_gui();
    } else {
        warn!("[微澜] 检测到已有另一个活跃的输入法守护进程正在运行，当前进程将安全退出。");
    }
}

/// 决策并派发 Wayland 或 IBus 前端会话
fn dispatch_active_session(
    opts: DaemonOptions,
    cfg: AppConfig,
    engine: waylan_algo::AlgoEngine,
    coordinator: ShutdownCoordinator,
) -> Result<(), DaemonError> {
    let caps = check_wayland_capabilities();
    let use_ibus = opts.ibus_backend || (!caps.has_input_method_v2 && !caps.has_input_method_v1);
    let (watched_paths, initial_mtime) = collect_watched_paths();
    let (should_spawn_tray, exclusive_source) = determine_tray_and_exclusive(&cfg, use_ibus);

    let tray = if should_spawn_tray {
        Some(tray::TrayService::spawn(coordinator.clone()))
    } else {
        None
    };

    if use_ibus {
        dispatch_ibus_session(IbusDispatchOptions {
            engine,
            is_ibus_arg: opts.ibus_backend,
            should_spawn_tray,
            exclusive_source,
            watched_paths,
            initial_mtime,
            tray: tray.as_ref(),
            coordinator,
        })
    } else {
        let theme = match cfg.ui.theme {
            ThemeMode::Light => UiTheme::light(),
            ThemeMode::Dark => UiTheme::dark(),
        };
        let mut renderer = CandidateBoxRenderer::new(theme);
        renderer.font_size = cfg.ui.font_size;

        dispatch_wayland_session(WaylandDispatchOptions {
            engine,
            renderer,
            cfg: &cfg,
            caps: &caps,
            watched_paths,
            initial_mtime,
            tray: tray.as_ref(),
            coordinator: &coordinator,
        })
    }
}
