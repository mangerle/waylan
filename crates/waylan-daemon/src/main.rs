use log::{info, warn};
use std::env;
use std::process;

use waylan_core::config::AppConfig;
use waylan_daemon::cli::{self, CliCommand, DaemonOptions};
use waylan_daemon::error::DaemonError;
use waylan_daemon::session_bundle::{
    build_algo_engine, check_wayland_capabilities, dispatch_ibus_session, dispatch_wayland_session,
    IbusDispatchOptions, WaylandDispatchOptions,
};
use waylan_daemon::system::{
    check_another_instance_running, collect_watched_paths, determine_tray_should_spawn,
    launch_settings_gui, try_acquire_single_instance_lock, ShutdownCoordinator, SingleInstanceLock,
};
use waylan_daemon::{desktop, logger, tray};
use waylan_ibus::{print_ibus_component_xml, print_ibus_engines_xml};
use waylan_sys::LoopWakeup;
use waylan_ui::{CandidateBoxRenderer, UiTheme};

fn main() {
    if let Err(e) = run() {
        eprintln!("错误: {}", e);
        process::exit(1);
    }
}

fn run() -> Result<(), DaemonError> {
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
        CliCommand::Daemon(opts) => run_daemon(opts),
    }
}

/// 判定当前进程是否处于系统登录管理器 (Greeter) 或受限系统级会话中
///
/// # 设计原理
/// - 微澜属于用户级图形桌面输入法，绝不应当在 GDM / LightDM 等系统登录管理器的密码界面驻留，
///   避免与登录界面原生英文密码输入产生状态冲突。
fn is_greeter_or_system_session() -> bool {
    // 1. UID < 1000 判定（系统服务用户，如 gdm、lightdm、systemd 等）
    if rustix::process::getuid().as_raw() < 1000 {
        return true;
    }

    // 2. 环境变量判定：XDG_SESSION_CLASS 明确标记为 greeter 或 lock-screen
    if let Ok(class) = env::var("XDG_SESSION_CLASS") {
        if class.eq_ignore_ascii_case("greeter") || class.eq_ignore_ascii_case("lock-screen") {
            return true;
        }
    }

    // 3. 用户名或会话名称敏感词过滤
    for var in &["USER", "USERNAME", "LOGNAME", "GDMSESSION"] {
        if let Ok(val) = env::var(var) {
            let lower = val.to_ascii_lowercase();
            if lower.contains("gdm")
                || lower.contains("greeter")
                || lower.contains("lightdm")
                || lower.contains("sddm")
            {
                return true;
            }
        }
    }

    false
}

/// 启动并运行输入法常驻守护服务
fn run_daemon(opts: DaemonOptions) -> Result<(), DaemonError> {
    if let Err(e) = logger::init_logger() {
        eprintln!("[微澜] 日志系统初始化失败: {}", e);
    }

    if is_greeter_or_system_session() {
        info!("[微澜守护进程] 检测到当前处于登录管理器或系统受限会话 (Greeter)，微澜静默退出以保障系统密码输入纯净。");
        return Ok(());
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
                info!("[微澜] 输入法守护进程已在后台常驻运行，正在为您唤起偏好设置中心并激活输入法...");
                activate_desktop_input_source();
                let _ = launch_settings_gui();
                return Ok(None);
            }
            Ok(None)
        }
    }
}

/// 处理单实例冲突唤起逻辑（激活输入法并唤起设置中心）
fn handle_instance_conflict(opts: &DaemonOptions) {
    if !opts.ibus_backend && !opts.self_check {
        info!("[微澜] 输入法守护进程已在后台常驻运行，正在为您唤起偏好设置中心并激活输入法...");
        activate_desktop_input_source();
        let _ = launch_settings_gui();
    } else {
        warn!("[微澜] 检测到已有另一个活跃的输入法守护进程正在运行，当前进程将安全退出。");
    }
}

/// 激活当前桌面会话的微澜输入源并置顶为首选
fn activate_desktop_input_source() {
    let is_gnome = env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false);

    if is_gnome {
        desktop::ensure_gnome_input_source_registered(false);
    }

    let mut cmd = process::Command::new("ibus");
    cmd.args(["engine", "waylan"]);
    let _ = desktop::run_command_with_timeout(cmd, desktop::DEFAULT_DESKTOP_COMMAND_TIMEOUT);
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
    let should_spawn_tray = determine_tray_should_spawn(&cfg, use_ibus);

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
            watched_paths,
            initial_mtime,
            tray: tray.as_ref(),
            coordinator,
        })
    } else {
        let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
        renderer.apply_ui_config(&cfg.ui, cfg.ui.theme);

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
