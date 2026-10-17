mod cli;
mod error;
mod logger;
mod session_bundle;
mod system;
mod tray;

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
    launch_settings_gui, try_acquire_single_instance_lock, ShutdownCoordinator,
};
pub use waylan_core::config;
use waylan_core::config::{AppConfig, ThemeMode};
use waylan_core::LoopWakeup;
use waylan_ibus::{print_ibus_component_xml, print_ibus_engines_xml};
use waylan_ui::{CandidateBoxRenderer, UiTheme};

/// 处理早期 CLI 选项（版本、XML 描述、偏好设置、帮助与词库子命令）
fn handle_early_cli(args: &[String]) -> Result<Option<()>, DaemonError> {
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V" || args[1] == "-v") {
        println!("waylan {}", env!("CARGO_PKG_VERSION"));
        return Ok(Some(()));
    }
    if args.len() >= 2 && (args[1] == "--xml" || args[1] == "--ibus-xml") {
        print_ibus_engines_xml();
        return Ok(Some(()));
    }
    if args.len() >= 2 && (args[1] == "--ibus-component-xml" || args[1] == "--component-xml") {
        print_ibus_component_xml();
        return Ok(Some(()));
    }
    if args.len() >= 2 && (args[1] == "settings" || args[1] == "--settings") {
        launch_settings_gui()?;
        return Ok(Some(()));
    }
    if args.len() >= 2 && (args[1] == "--help" || args[1] == "-h" || args[1] == "help") {
        cli::print_main_usage();
        return Ok(Some(()));
    }
    if args.len() >= 2 && args[1] == "dict" {
        cli::handle_dict_subcommand(&args[2..])?;
        return Ok(Some(()));
    }
    Ok(None)
}

fn main() -> Result<(), DaemonError> {
    let args: Vec<String> = env::args().collect();
    if let Some(()) = handle_early_cli(&args)? {
        return Ok(());
    }

    if let Err(e) = logger::init_logger() {
        eprintln!("[微澜] 日志系统初始化失败: {}", e);
    }

    let is_ibus_arg = args.len() >= 2 && (args[1] == "--ibus" || args[1] == "ibus");
    if args.len() >= 2 && args[1] != "--check" && !is_ibus_arg {
        eprintln!("错误: 未知的命令行参数 '{}'", args[1]);
        cli::print_main_usage();
        process::exit(2);
    }

    let _single_instance_lock = match try_acquire_single_instance_lock() {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            if !is_ibus_arg && (args.len() == 1 || (args.len() == 2 && args[1] != "--check")) {
                info!("[微澜] 输入法守护进程已在后台常驻运行，正在为您唤起偏好设置中心...");
                let _ = launch_settings_gui();
            } else {
                warn!("[微澜] 检测到已有另一个活跃的输入法守护进程正在运行，当前进程将安全退出。");
            }
            return Ok(());
        }
        Err(e) => {
            warn!(
                "[微澜] 获取单例运行文件锁遇到异常 ({})，回退至进程扫描校验...",
                e
            );
            if !is_ibus_arg
                && (args.len() == 1 || (args.len() == 2 && args[1] != "--check"))
                && check_another_instance_running()
            {
                info!("[微澜] 输入法守护进程已在后台常驻运行，正在为您唤起偏好设置中心...");
                let _ = launch_settings_gui();
                return Ok(());
            }
            None
        }
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

    if args.len() > 1 && args[1] == "--check" {
        cli::run_self_check(&mut engine)?;
        return Ok(());
    }

    if coordinator.is_shutting_down() {
        return Ok(());
    }

    let caps = check_wayland_capabilities();
    let use_ibus_backend = is_ibus_arg || (!caps.has_input_method_v2 && !caps.has_input_method_v1);
    let (watched_paths, initial_mtime) = collect_watched_paths();
    let (should_spawn_tray, exclusive_source) =
        determine_tray_and_exclusive(&cfg, use_ibus_backend);

    let tray = if should_spawn_tray {
        Some(tray::TrayService::spawn(coordinator.clone()))
    } else {
        None
    };

    if use_ibus_backend {
        dispatch_ibus_session(IbusDispatchOptions {
            engine,
            is_ibus_arg,
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
