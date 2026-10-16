mod cli;
mod config;
mod ibus;
mod logger;
mod session_bundle;
mod tray;

use config::{AppConfig, SwitchKey, ThemeMode};
use log::{info, warn};
use session_bundle::{
    apply_reload_v1, apply_reload_v2, check_wayland_capabilities, load_auxiliary_managers,
    load_lexicon_table,
};
use std::env;
use std::error::Error;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};
use waylan_algo::types::{AlgoConfig, FuzzyConfig};
use waylan_algo::AlgoEngine;
use waylan_ui::{CandidateBoxRenderer, UiTheme};
use waylan_wayland::session_v1::WaylanV1Session;
use waylan_wayland::session_v2::WaylanV2Session;
use waylan_wayland::{
    run_session_v1_loop, run_session_v2_loop_fallible, LoopWakeup, WaylandCapabilities,
};

/// 处理早期 CLI 选项（版本、XML 描述、偏好设置、帮助与词库子命令）
fn handle_early_cli(args: &[String]) -> Result<Option<()>, Box<dyn Error>> {
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V" || args[1] == "-v") {
        println!("waylan {}", env!("CARGO_PKG_VERSION"));
        return Ok(Some(()));
    }
    if args.len() >= 2 && (args[1] == "--xml" || args[1] == "--ibus-xml") {
        ibus::print_ibus_engines_xml();
        return Ok(Some(()));
    }
    if args.len() >= 2 && (args[1] == "--ibus-component-xml" || args[1] == "--component-xml") {
        ibus::print_ibus_component_xml();
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

/// 注册进程系统退出信号拦截 (SIGINT / SIGTERM)
fn register_system_signals(term_now: &Arc<AtomicBool>) {
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        if let Err(e) =
            signal_hook::flag::register_conditional_shutdown(sig, 1, Arc::clone(term_now))
        {
            warn!("[微澜守护进程] 注册信号拦截失败: {}", e);
        }
        if let Err(e) = signal_hook::flag::register(sig, Arc::clone(term_now)) {
            warn!("[微澜守护进程] 注册信号标志置位失败: {}", e);
        }
    }
}

/// 构建算法引擎并加载全量汉字库、自定义短语与用户个性化词库
fn build_algo_engine(
    cfg: &AppConfig,
    term_now: &Arc<AtomicBool>,
) -> Result<Option<AlgoEngine>, Box<dyn Error>> {
    let table = load_lexicon_table();
    if term_now.load(Ordering::Relaxed) {
        info!("[微澜守护进程] 启动加载期间收到终止信号，立即停止启动并安全退场。");
        return Ok(None);
    }
    let algo_config = AlgoConfig {
        page_size: cfg.page_size,
        enable_v_mode: cfg.v_mode,
        enable_u_mode: cfg.enable_u_mode,
        enable_i_mode: cfg.enable_i_mode,
        traditional_chinese: cfg.traditional_chinese,
        filter_uncommon_charset: cfg.filter_uncommon_charset,
        show_pinyin_comment: cfg.show_pinyin_comment,
        auto_dedup: cfg.auto_dedup,
        fuzzy: if cfg.fuzzy_pinyin {
            FuzzyConfig::all_enabled()
        } else {
            FuzzyConfig::default()
        },
        auto_pangu_spacing: cfg.auto_pangu_spacing,
    };
    let mut engine = AlgoEngine::new(table, algo_config);

    let phrase_path = CustomPhraseManager::default_path();
    engine.custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);
    let user_dict = AppConfig::user_dict_path();
    engine.set_user_dict_path(&user_dict);
    if user_dict.exists() {
        info!(
            "[字库引擎] 成功载入用户自适应个性化历史词库: {:?} (词条数: {})",
            user_dict,
            engine.user_model.total_entries()
        );
    }

    let (english_suggester, symbol_suggester) = load_auxiliary_managers();
    engine.english_suggester = english_suggester;
    engine.symbol_suggester = symbol_suggester;

    info!(
        "[简繁词库] 全局简繁转换两级词库就绪 (单字映射: {} 个, 消歧短语: {} 组)",
        engine.opencc_dict.chars.len(),
        engine.opencc_dict.phrases_len()
    );

    Ok(Some(engine))
}

/// 收集被监控热重载的配置文件与词库路径及其初始修改时间
fn collect_watched_paths() -> (Vec<PathBuf>, Option<SystemTime>) {
    let mut watched_paths = Vec::with_capacity(3);
    if let Some(c_path) = AppConfig::config_path() {
        watched_paths.push(c_path);
    }
    watched_paths.push(CustomPhraseManager::default_path());
    watched_paths.push(AppConfig::user_aux_dict_path());

    let initial_mtime = watched_paths
        .iter()
        .filter_map(|p| fs::metadata(p).ok().and_then(|m| m.modified().ok()))
        .max();
    (watched_paths, initial_mtime)
}

/// 计算托盘展示形态与独占输入源配置
fn determine_tray_and_exclusive(cfg: &AppConfig, use_ibus_backend: bool) -> (bool, bool) {
    let is_gnome = env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false);
    let force_tray = cfg.enable_tray.unwrap_or_else(|| {
        env::var("WAYLAN_ENABLE_TRAY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    });
    let exclusive_source = cfg.exclusive_input_source
        || env::var("WAYLAN_EXCLUSIVE_INPUT_SOURCE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    let should_spawn_tray = force_tray || (!use_ibus_backend && !is_gnome);
    (should_spawn_tray, exclusive_source)
}

/// IBus 会话派发参数包
struct IbusDispatchOptions<'a> {
    engine: AlgoEngine,
    is_ibus_arg: bool,
    should_spawn_tray: bool,
    exclusive_source: bool,
    watched_paths: Vec<PathBuf>,
    initial_mtime: Option<SystemTime>,
    tray: Option<&'a tray::TrayService>,
    term_now: Arc<AtomicBool>,
    loop_wakeup: Option<LoopWakeup>,
}

/// 运行 IBus 会话并统一收敛异常退出与停机退出码
fn dispatch_ibus_session(opts: IbusDispatchOptions<'_>) -> Result<(), Box<dyn Error>> {
    info!(
        "[微澜守护进程] 启用 IBus / D-Bus 输入法引擎会话 (展示形态: {}, 原生指示器属性上报就绪)。",
        if opts.should_spawn_tray {
            "形态 B 专属彩色托盘"
        } else {
            "形态 A 原生顶栏"
        }
    );
    let ibus_res = ibus::run_ibus_session(ibus::IbusSessionOptions {
        engine: opts.engine,
        is_ibus_child_mode: opts.is_ibus_arg,
        watched_paths: opts.watched_paths,
        initial_mtime: opts.initial_mtime,
        shared_is_chinese: opts.tray.as_ref().map(|t| t.mode_flag()),
        shared_is_caps_locked: opts.tray.as_ref().map(|t| t.caps_lock_flag()),
        shared_traditional_chinese: opts.tray.as_ref().map(|t| t.traditional_chinese_flag()),
        shared_full_width_punct: opts.tray.as_ref().map(|t| t.full_width_punct_flag()),
        shared_full_width_char: opts.tray.as_ref().map(|t| t.full_width_char_flag()),
        mode_notify_tx: opts.tray.as_ref().map(|t| t.icon_notifier()),
        reload_requested: opts.tray.as_ref().map(|t| t.reload_flag()),
        exit_requested: Arc::clone(&opts.term_now),
        loop_wakeup: opts.loop_wakeup,
        suppress_properties: false,
        exclusive_input_source: opts.exclusive_source,
    });
    match ibus_res {
        Ok(_) => {
            info!("[微澜守护进程] IBus 输入法事件循环已正常退出，所有资源已安全释放。");
            Ok(())
        }
        Err(_) if opts.term_now.load(Ordering::Relaxed) => {
            info!("[微澜守护进程] 收到退出信号，IBus 服务安全退场。");
            Ok(())
        }
        Err(e) => {
            log::error!("[微澜守护进程 错误] IBus 输入法会话异常退出: {}", e);
            process::exit(1);
        }
    }
}

/// Wayland 内部会话参数包
struct WaylandSessionParams<'a> {
    engine: AlgoEngine,
    renderer: CandidateBoxRenderer,
    cfg: &'a AppConfig,
    watched_paths: Vec<PathBuf>,
    initial_mtime: Option<SystemTime>,
    tray: Option<&'a tray::TrayService>,
    loop_wakeup: Option<LoopWakeup>,
}

/// 运行 Wayland V1 会话
fn run_v1_session(params: WaylandSessionParams<'_>) -> Result<(), Box<dyn Error>> {
    let mut session = WaylanV1Session::with_engine(params.engine);
    session.surface_manager.renderer = params.renderer;
    session.watched_paths = params.watched_paths;
    session.last_watched_mtime = params.initial_mtime;
    session.shift_sm = ShiftToggleStateMachine::with_toggle_key(match params.cfg.switch_key {
        SwitchKey::Shift => ToggleKeyMode::Shift,
        SwitchKey::Ctrl => ToggleKeyMode::Ctrl,
    });
    session.punctuation_processor = PunctuationProcessor::with_options(
        params.cfg.slash_to_dunhao,
        params.cfg.protect_digits_punctuation,
        params.cfg.auto_complete_pairs,
    );
    session.page_keys = params.cfg.page_keys.into();
    session.shared_is_chinese = params.tray.as_ref().map(|t| t.mode_flag());
    session.shared_is_caps_locked = params.tray.as_ref().map(|t| t.caps_lock_flag());
    session.mode_notify_tx = params.tray.as_ref().map(|t| t.icon_notifier());
    session.reload_requested = params.tray.as_ref().map(|t| t.reload_flag());
    session.exit_requested = params.tray.as_ref().map(|t| t.exit_flag());
    session.loop_wakeup = params.loop_wakeup;
    session.reload_callback = Some(Box::new(apply_reload_v1));
    run_session_v1_loop(session).map_err(Into::into)
}

/// 运行 Wayland V2 会话，并在协议失败时自动降级至 V1 会话
fn run_v2_session_with_v1_fallback(params: WaylandSessionParams<'_>) -> Result<(), Box<dyn Error>> {
    info!("[微澜守护进程] 检测到 zwp_input_method_manager_v2 支持，启用现代 Wayland V2 协议会话。");
    let mut session_v2 = WaylanV2Session::new(params.engine);
    session_v2.surface_manager.renderer = params.renderer;
    session_v2.watched_paths = params.watched_paths;
    session_v2.last_watched_mtime = params.initial_mtime;
    session_v2.shift_state =
        ShiftToggleStateMachine::with_toggle_key(match params.cfg.switch_key {
            SwitchKey::Shift => ToggleKeyMode::Shift,
            SwitchKey::Ctrl => ToggleKeyMode::Ctrl,
        });
    let mut session_v2_punct = PunctuationProcessor::with_options(
        params.cfg.slash_to_dunhao,
        params.cfg.protect_digits_punctuation,
        params.cfg.auto_complete_pairs,
    );
    session_v2_punct.full_width_punct = params.cfg.full_width_punct;
    session_v2.punctuation_processor = session_v2_punct;
    session_v2.page_keys = params.cfg.page_keys.into();
    session_v2.shared_is_chinese = params.tray.as_ref().map(|t| t.mode_flag());
    session_v2.shared_is_caps_locked = params.tray.as_ref().map(|t| t.caps_lock_flag());
    session_v2.mode_notify_tx = params.tray.as_ref().map(|t| t.icon_notifier());
    session_v2.reload_requested = params.tray.as_ref().map(|t| t.reload_flag());
    session_v2.exit_requested = params.tray.as_ref().map(|t| t.exit_flag());
    session_v2.loop_wakeup = params.loop_wakeup.clone();
    session_v2.reload_callback = Some(Box::new(apply_reload_v2));
    let v2_result = run_session_v2_loop_fallible(session_v2);
    match v2_result {
        Ok(()) => Ok(()),
        Err((e, returned_session)) => {
            if params
                .tray
                .as_ref()
                .map(|t| t.is_exit_requested())
                .unwrap_or(false)
            {
                info!("[微澜守护进程] 检测到优雅退出请求，服务安全退场。");
                return Ok(());
            }
            warn!(
                "[微澜守护进程] Wayland V2 协议会话不可用或异常结束 ({})，正在自动降级启动 V1 协议会话...",
                e
            );
            let returned_session = *returned_session;
            let mut surface_manager = returned_session.surface_manager;
            surface_manager.clear_wayland_resources();
            let mut session = WaylanV1Session::with_engine(returned_session.engine);
            session.surface_manager = surface_manager;
            session.keymap_layout = returned_session.keymap_layout;
            session.watched_paths = returned_session.watched_paths;
            session.last_watched_mtime = returned_session.last_watched_mtime;
            session.shift_sm = returned_session.shift_state;
            session.punctuation_processor = returned_session.punctuation_processor;
            session.page_keys = returned_session.page_keys;
            session.shared_is_chinese = params.tray.as_ref().map(|t| t.mode_flag());
            session.shared_is_caps_locked = params.tray.as_ref().map(|t| t.caps_lock_flag());
            session.mode_notify_tx = params.tray.as_ref().map(|t| t.icon_notifier());
            session.reload_requested = params.tray.as_ref().map(|t| t.reload_flag());
            session.exit_requested = params.tray.as_ref().map(|t| t.exit_flag());
            session.loop_wakeup = params.loop_wakeup;
            session.reload_callback = Some(Box::new(apply_reload_v1));
            run_session_v1_loop(session).map_err(Into::into)
        }
    }
}

/// Wayland 会话派发参数包
struct WaylandDispatchOptions<'a> {
    engine: AlgoEngine,
    renderer: CandidateBoxRenderer,
    cfg: &'a AppConfig,
    caps: &'a WaylandCapabilities,
    watched_paths: Vec<PathBuf>,
    initial_mtime: Option<SystemTime>,
    tray: Option<&'a tray::TrayService>,
    term_now: &'a Arc<AtomicBool>,
    loop_wakeup: Option<LoopWakeup>,
}

/// 运行 Wayland 会话并处理异常退出
fn dispatch_wayland_session(opts: WaylandDispatchOptions<'_>) -> Result<(), Box<dyn Error>> {
    let session_params = WaylandSessionParams {
        engine: opts.engine,
        renderer: opts.renderer,
        cfg: opts.cfg,
        watched_paths: opts.watched_paths,
        initial_mtime: opts.initial_mtime,
        tray: opts.tray,
        loop_wakeup: opts.loop_wakeup,
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
            if opts
                .tray
                .as_ref()
                .map(|t| t.is_exit_requested())
                .unwrap_or(false)
                || opts.term_now.load(Ordering::Relaxed)
            {
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

fn main() -> Result<(), Box<dyn Error>> {
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

    if !is_ibus_arg
        && (args.len() == 1 || (args.len() == 2 && args[1] != "--check"))
        && check_another_instance_running()
    {
        info!("[微澜] 输入法守护进程已在后台常驻运行，正在为您唤起偏好设置中心...");
        let _ = launch_settings_gui();
        return Ok(());
    }

    let term_now = Arc::new(AtomicBool::new(false));
    register_system_signals(&term_now);

    info!("============================================================");
    info!(
        "  微澜 (WayLan) - 现代轻量纯 Rust 中文输入法服务 (版本: {})",
        env!("CARGO_PKG_VERSION")
    );
    info!("============================================================");

    let cfg = AppConfig::load_or_create();
    let theme = match cfg.theme {
        ThemeMode::Light => UiTheme::light(),
        ThemeMode::Dark => UiTheme::dark(),
    };
    let mut renderer = CandidateBoxRenderer::new(theme);
    renderer.font_size = cfg.font_size;

    let Some(mut engine) = build_algo_engine(&cfg, &term_now)? else {
        return Ok(());
    };

    if args.len() > 1 && args[1] == "--check" {
        cli::run_self_check(&mut engine)?;
        return Ok(());
    }

    if term_now.load(Ordering::Relaxed) {
        return Ok(());
    }

    let caps = check_wayland_capabilities();
    let loop_wakeup = LoopWakeup::new().ok();
    let use_ibus_backend = is_ibus_arg || (!caps.has_input_method_v2 && !caps.has_input_method_v1);
    let (watched_paths, initial_mtime) = collect_watched_paths();
    let (should_spawn_tray, exclusive_source) =
        determine_tray_and_exclusive(&cfg, use_ibus_backend);

    let tray = if should_spawn_tray {
        Some(tray::TrayService::spawn(
            Arc::clone(&term_now),
            loop_wakeup.clone(),
        ))
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
            term_now,
            loop_wakeup,
        })
    } else {
        dispatch_wayland_session(WaylandDispatchOptions {
            engine,
            renderer,
            cfg: &cfg,
            caps: &caps,
            watched_paths,
            initial_mtime,
            tray: tray.as_ref(),
            term_now: &term_now,
            loop_wakeup,
        })
    }
}

/// 唤起独立的微澜偏好设置中心图形界面
///
/// # 设计原理
/// - **实现初衷**：统一 CLI `settings` 子命令与桌面单例防多开时的偏好设置界面唤起逻辑。
/// - **核心优势**：自动继承当前 Wayland/X11 会话与 D-Bus 上下文，无缝拉起 Slint 设置中心。
/// - **代价与局限**：若目标可执行文件缺失则返回对应 I/O 错误。
fn launch_settings_gui() -> Result<(), Box<dyn Error>> {
    info!("[微澜守护进程] 正在拉起独立的偏好设置中心 (waylan-settings)...");
    let current_exe = env::current_exe().ok();
    let sibling_settings = current_exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join("waylan-settings"))
        .filter(|p| p.is_file());

    let mut cmd = if let Some(ref path) = sibling_settings {
        Command::new(path)
    } else {
        Command::new("waylan-settings")
    };

    for var in [
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_CURRENT_DESKTOP",
    ] {
        if let Ok(val) = env::var(var) {
            cmd.env(var, &val);
        }
    }

    cmd.stdin(Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);

    let _child = cmd.spawn()?;
    Ok(())
}

/// 检查当前系统桌面会话中是否已有活跃的微澜输入法守护进程正在运行
///
/// # 设计原理
/// - **实现初衷**：防止用户从桌面应用菜单重复点击“微澜输入法”导致多实例冲突与托盘抢占失败。
/// - **核心优势**：通过 Linux 原生 `/proc` 文件系统快速遍历 `comm` 并比对属主 UID，微秒级判定且杜绝多用户误判。
/// - **代价与局限**：仅适用于 Linux 操作系统环境。
fn check_another_instance_running() -> bool {
    #[cfg(target_os = "linux")]
    {
        let current_pid = process::id();
        let current_uid = rustix::process::getuid().as_raw();
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Ok(pid) = file_name.parse::<u32>() else {
                    continue;
                };
                if pid == current_pid {
                    continue;
                }
                // 仅匹配同属当前用户的实例，杜绝 Linux 多用户环境下的误判拦截
                if let Ok(meta) = entry.metadata() {
                    if meta.uid() != current_uid {
                        continue;
                    }
                }
                if let Ok(comm) = fs::read_to_string(path.join("comm")) {
                    if comm.trim() == "waylan" {
                        return true;
                    }
                }
            }
        }
    }
    false
}
