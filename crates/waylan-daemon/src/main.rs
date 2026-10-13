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
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};
use waylan_algo::types::{AlgoConfig, FuzzyConfig};
use waylan_algo::AlgoEngine;
use waylan_ui::{CandidateBoxRenderer, UiTheme};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    // 0. 处理版本查询参数与 IBus XML 引擎描述查询（需在日志输出至 stdout 前完成）
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V" || args[1] == "-v") {
        println!("waylan {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.len() >= 2 && (args[1] == "--xml" || args[1] == "--ibus-xml") {
        ibus::print_ibus_engines_xml();
        return Ok(());
    }

    // 1. 初始化双通道日志系统（终端输出 + 文件落盘），确保后续 CLI 子命令与主服务日志正常输出
    let log_init_res = logger::init_logger();
    match log_init_res {
        Ok(ref path) => {
            info!("微澜 (WayLan) 日志系统初始化完成，持久化文件: {:?}", path);
        }
        Err(ref e) => {
            eprintln!("[微澜] 日志系统初始化失败: {}", e);
        }
    }

    // 2. 处理 settings 子命令（透明转发拉起独立的 waylan-settings 偏好设置客户端）
    if args.len() >= 2 && (args[1] == "settings" || args[1] == "--settings") {
        info!("[微澜守护进程] 正在拉起独立的偏好设置中心 (waylan-settings)...");
        let current_exe = env::current_exe().ok();
        let sibling_settings = current_exe
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.join("waylan-settings"))
            .filter(|p| p.is_file());

        let mut cmd = if let Some(ref path) = sibling_settings {
            std::process::Command::new(path)
        } else {
            std::process::Command::new("waylan-settings")
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

        match cmd.spawn() {
            Ok(_) => {
                info!("[微澜守护进程] 偏好设置中心已成功唤起。");
                return Ok(());
            }
            Err(e) => {
                eprintln!(
                    "[微澜守护进程] 启动偏好设置中心失败: {}。请确认 waylan-settings 已正确安装。",
                    e
                );
                std::process::exit(1);
            }
        }
    }

    // 3. 处理帮助查询参数
    if args.len() >= 2 && (args[1] == "--help" || args[1] == "-h" || args[1] == "help") {
        cli::print_main_usage();
        return Ok(());
    }

    // 4. 处理 dict 子命令（支持词库打包与用户自定义词库导入）
    if args.len() >= 2 && args[1] == "dict" {
        return cli::handle_dict_subcommand(&args[2..]);
    }

    let is_ibus_arg = args.len() >= 2 && (args[1] == "--ibus" || args[1] == "ibus");

    // 处理自检参数或未知参数检查（排除合法的无参启动或合法选项）
    if args.len() >= 2 && args[1] != "--check" && !is_ibus_arg {
        eprintln!("错误: 未知的命令行参数 '{}'", args[1]);
        cli::print_main_usage();
        std::process::exit(2);
    }

    // 注册进程系统退出信号拦截 (SIGINT / SIGTERM)：
    // 首次收到信号时将 term_now 置为 true 请求平滑退出；若再次收到相同退出信号则立即退出 (exit 0) 避免进程陷入无法中断的僵死状态
    let term_now = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Err(e) = signal_hook::flag::register_conditional_shutdown(
        signal_hook::consts::SIGTERM,
        1,
        std::sync::Arc::clone(&term_now),
    ) {
        warn!("[微澜守护进程] 注册 SIGTERM 信号拦截失败: {}", e);
    }
    if let Err(e) = signal_hook::flag::register(
        signal_hook::consts::SIGTERM,
        std::sync::Arc::clone(&term_now),
    ) {
        warn!("[微澜守护进程] 注册 SIGTERM 标志置位失败: {}", e);
    }

    if let Err(e) = signal_hook::flag::register_conditional_shutdown(
        signal_hook::consts::SIGINT,
        1,
        std::sync::Arc::clone(&term_now),
    ) {
        warn!("[微澜守护进程] 注册 SIGINT 信号拦截失败: {}", e);
    }
    if let Err(e) = signal_hook::flag::register(
        signal_hook::consts::SIGINT,
        std::sync::Arc::clone(&term_now),
    ) {
        warn!("[微澜守护进程] 注册 SIGINT 标志置位失败: {}", e);
    }

    info!("============================================================");
    info!(
        "  微澜 (WayLan) - 现代轻量纯 Rust 中文输入法服务 (版本: {})",
        env!("CARGO_PKG_VERSION")
    );
    info!("============================================================");

    // 3. 加载或初始化配置
    let cfg = AppConfig::load_or_create();
    info!(
        "[配置系统] 成功加载全局配置：中英文切换键={:?}, 翻页键={:?}, 单页候选数={}, 候选字号={}px, 主题={:?}",
        cfg.switch_key, cfg.page_keys, cfg.page_size, cfg.font_size, cfg.theme
    );
    info!(
        "[配置系统] 符号与算法参数：顿号替换={}, 数字保护={}, 成对补全={}, V模式={}, 模糊音={}, 繁体={}",
        cfg.slash_to_dunhao,
        cfg.protect_digits_punctuation,
        cfg.auto_complete_pairs,
        cfg.v_mode,
        cfg.fuzzy_pinyin,
        cfg.traditional_chinese
    );

    // 4. 初始化 UI 渲染引擎
    let theme = match cfg.theme {
        ThemeMode::Light => UiTheme::light(),
        ThemeMode::Dark => UiTheme::dark(),
    };
    let mut renderer = CandidateBoxRenderer::new(theme);
    renderer.font_size = cfg.font_size;
    info!("[界面引擎] 候选框矢量渲染器初始化完成（单行胶囊对标样式就绪）。");

    // 5. 加载全量汉字库与词库
    let table = load_lexicon_table();
    if term_now.load(std::sync::atomic::Ordering::Relaxed) {
        info!("[微澜守护进程] 启动加载期间收到终止信号，立即停止启动并安全退场。");
        return Ok(());
    }
    let algo_config = AlgoConfig {
        page_size: cfg.page_size,
        enable_v_mode: cfg.v_mode,
        traditional_chinese: cfg.traditional_chinese,
        fuzzy: if cfg.fuzzy_pinyin {
            FuzzyConfig::all_enabled()
        } else {
            FuzzyConfig::default()
        },
        auto_pangu_spacing: cfg.auto_pangu_spacing,
        ..Default::default()
    };
    let mut engine = AlgoEngine::new(table, algo_config);

    // 加载自定义短语与用户历史自造词库
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

    // 载入混输与辅助符号词库 (aux.dict.bin)
    let (english_suggester, symbol_suggester) = load_auxiliary_managers();
    engine.english_suggester = english_suggester;
    engine.symbol_suggester = symbol_suggester;

    // 简繁转换两级词库已在 AlgoEngine 构造时通过 OpenCcDict::global() 完成全局单例加载
    info!(
        "[简繁词库] 全局简繁转换两级词库就绪 (单字映射: {} 个, 消歧短语: {} 组)",
        engine.opencc_dict.chars.len(),
        engine.opencc_dict.phrases_len()
    );

    // 6. 执行环境自检（--check 参数，在词库加载完成后立即自检，支持无 Wayland GUI 环境下的 CI 测试）
    if args.len() > 1 && args[1] == "--check" {
        cli::run_self_check(&mut engine)?;
        return Ok(());
    }

    if term_now.load(std::sync::atomic::Ordering::Relaxed) {
        info!("[微澜守护进程] 启动加载期间收到终止信号，立即停止启动并安全退场。");
        return Ok(());
    }

    // 7. 探测 Wayland 环境与合成器协议能力
    let caps = check_wayland_capabilities();

    if term_now.load(std::sync::atomic::Ordering::Relaxed) {
        info!("[微澜守护进程] 探测环境期间收到终止信号，立即安全退场。");
        return Ok(());
    }

    // 8. 启动事件循环跨线程唤醒通道
    let loop_wakeup = waylan_wayland::LoopWakeup::new().ok();
    let use_ibus_backend = is_ibus_arg || (!caps.has_input_method_v2 && !caps.has_input_method_v1);

    info!("------------------------------------------------------------");
    info!("[微澜守护进程] 核心服务启动就绪，正在准备建立 Wayland 输入法会话。");
    info!("------------------------------------------------------------");

    let mut watched_paths = Vec::with_capacity(3);
    if let Some(c_path) = AppConfig::config_path() {
        watched_paths.push(c_path);
    }
    watched_paths.push(CustomPhraseManager::default_path());
    watched_paths.push(AppConfig::user_aux_dict_path());

    let initial_mtime = watched_paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().and_then(|m| m.modified().ok()))
        .max();

    // 9. 智能判定是否需要拉起独立的 StatusNotifierItem 托盘：
    // 在 GNOME 桌面环境的 IBus 会话中：
    // - 默认情况下（形态 A），GNOME Shell 顶栏原生集成输入源指示器，不拉起独立托盘，顶栏保持单一原生指示；
    // - 当用户开启专属彩色托盘（enable_tray=true 或 WAYLAN_ENABLE_TRAY=1，形态 B）时：
    //   拉起展示微澜青底白字矢量图标的专属托盘，同时对 IBus 引擎静默原生属性上报并置为独占输入源，
    //   促使 GNOME Shell 自动隐藏其原生白色指示器，实现顶栏“仅保留微澜专属彩色图标”。
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

    let tray = if should_spawn_tray {
        Some(tray::TrayService::spawn(
            std::sync::Arc::clone(&term_now),
            loop_wakeup.clone(),
        ))
    } else {
        None
    };

    if use_ibus_backend {
        let suppress_properties = should_spawn_tray;
        info!(
            "[微澜守护进程] 启用 IBus / D-Bus 输入法引擎会话 (展示形态: {}, 原生指示器静默: {})。",
            if should_spawn_tray {
                "形态 B 专属彩色托盘"
            } else {
                "形态 A 原生顶栏"
            },
            suppress_properties
        );
        let ibus_res = ibus::run_ibus_session(ibus::IbusSessionOptions {
            engine,
            is_ibus_child_mode: is_ibus_arg,
            watched_paths,
            initial_mtime,
            shared_is_chinese: tray.as_ref().map(|t| t.mode_flag()),
            shared_is_caps_locked: tray.as_ref().map(|t| t.caps_lock_flag()),
            shared_traditional_chinese: tray.as_ref().map(|t| t.traditional_chinese_flag()),
            shared_full_width_punct: tray.as_ref().map(|t| t.full_width_punct_flag()),
            shared_full_width_char: tray.as_ref().map(|t| t.full_width_char_flag()),
            mode_notify_tx: tray.as_ref().map(|t| t.icon_notifier()),
            reload_requested: tray.as_ref().map(|t| t.reload_flag()),
            exit_requested: std::sync::Arc::clone(&term_now),
            loop_wakeup,
            suppress_properties,
            exclusive_input_source: exclusive_source || should_spawn_tray,
        });
        return match ibus_res {
            Ok(_) => {
                info!("[微澜守护进程] IBus 输入法事件循环已正常退出，所有资源已安全释放。");
                Ok(())
            }
            Err(_) if term_now.load(std::sync::atomic::Ordering::Relaxed) => {
                info!("[微澜守护进程] 收到退出信号，IBus 服务安全退场。");
                Ok(())
            }
            Err(e) => {
                log::error!("[微澜守护进程 错误] IBus 输入法会话异常退出: {}", e);
                Err(e)
            }
        };
    }

    let loop_result = if caps.has_input_method_v2 && caps.has_seat {
        info!("[微澜守护进程] 检测到 zwp_input_method_manager_v2 支持，启用现代 Wayland V2 协议会话。");
        let mut session_v2 = waylan_wayland::session_v2::WaylanV2Session::new(engine);
        session_v2.surface_manager.renderer = renderer;
        session_v2.watched_paths = watched_paths;
        session_v2.last_watched_mtime = initial_mtime;
        session_v2.shift_state = ShiftToggleStateMachine::with_toggle_key(match cfg.switch_key {
            SwitchKey::Shift => ToggleKeyMode::Shift,
            SwitchKey::Ctrl => ToggleKeyMode::Ctrl,
        });
        let mut session_v2_punct = PunctuationProcessor::with_options(
            cfg.slash_to_dunhao,
            cfg.protect_digits_punctuation,
            cfg.auto_complete_pairs,
        );
        session_v2_punct.full_width_punct = cfg.full_width_punct;
        session_v2.punctuation_processor = session_v2_punct;
        session_v2.page_keys = cfg.page_keys.into();
        session_v2.shared_is_chinese = tray.as_ref().map(|t| t.mode_flag());
        session_v2.shared_is_caps_locked = tray.as_ref().map(|t| t.caps_lock_flag());
        session_v2.mode_notify_tx = tray.as_ref().map(|t| t.icon_notifier());
        session_v2.reload_requested = tray.as_ref().map(|t| t.reload_flag());
        session_v2.exit_requested = tray.as_ref().map(|t| t.exit_flag());
        session_v2.loop_wakeup = loop_wakeup.clone();
        session_v2.reload_callback = Some(Box::new(apply_reload_v2));
        let v2_result = waylan_wayland::run_session_v2_loop_fallible(session_v2);
        match v2_result {
            Ok(()) => Ok(()),
            Err((e, returned_session)) => {
                if tray
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
                let engine = returned_session.engine;
                let mut surface_manager = returned_session.surface_manager;
                surface_manager.clear_wayland_resources();
                let watched_paths = returned_session.watched_paths;
                let last_watched_mtime = returned_session.last_watched_mtime;
                let keymap_layout = returned_session.keymap_layout;
                let shift_sm = returned_session.shift_state;
                let punctuation_processor = returned_session.punctuation_processor;
                let page_keys = returned_session.page_keys;

                let mut session = waylan_wayland::session_v1::WaylanV1Session::with_engine(engine);
                session.surface_manager = surface_manager;
                session.keymap_layout = keymap_layout;
                session.watched_paths = watched_paths;
                session.last_watched_mtime = last_watched_mtime;
                session.shift_sm = shift_sm;
                session.punctuation_processor = punctuation_processor;
                session.page_keys = page_keys;
                session.shared_is_chinese = tray.as_ref().map(|t| t.mode_flag());
                session.shared_is_caps_locked = tray.as_ref().map(|t| t.caps_lock_flag());
                session.mode_notify_tx = tray.as_ref().map(|t| t.icon_notifier());
                session.reload_requested = tray.as_ref().map(|t| t.reload_flag());
                session.exit_requested = tray.as_ref().map(|t| t.exit_flag());
                session.loop_wakeup = loop_wakeup.clone();
                session.reload_callback = Some(Box::new(apply_reload_v1));
                waylan_wayland::run_session_v1_loop(session)
            }
        }
    } else {
        let mut session = waylan_wayland::session_v1::WaylanV1Session::with_engine(engine);
        session.surface_manager.renderer = renderer;
        session.watched_paths = watched_paths;
        session.last_watched_mtime = initial_mtime;
        session.shift_sm = ShiftToggleStateMachine::with_toggle_key(match cfg.switch_key {
            SwitchKey::Shift => ToggleKeyMode::Shift,
            SwitchKey::Ctrl => ToggleKeyMode::Ctrl,
        });
        session.punctuation_processor = PunctuationProcessor::with_options(
            cfg.slash_to_dunhao,
            cfg.protect_digits_punctuation,
            cfg.auto_complete_pairs,
        );
        session.page_keys = cfg.page_keys.into();
        session.shared_is_chinese = tray.as_ref().map(|t| t.mode_flag());
        session.shared_is_caps_locked = tray.as_ref().map(|t| t.caps_lock_flag());
        session.mode_notify_tx = tray.as_ref().map(|t| t.icon_notifier());
        session.reload_requested = tray.as_ref().map(|t| t.reload_flag());
        session.exit_requested = tray.as_ref().map(|t| t.exit_flag());
        session.loop_wakeup = loop_wakeup;
        session.reload_callback = Some(Box::new(apply_reload_v1));
        waylan_wayland::run_session_v1_loop(session)
    };

    match loop_result {
        Ok(_) => {
            info!("[微澜守护进程] Wayland 输入法事件循环已正常退出，所有资源已安全释放。");
        }
        Err(e) => {
            if tray
                .as_ref()
                .map(|t| t.is_exit_requested())
                .unwrap_or(false)
                || term_now.load(std::sync::atomic::Ordering::Relaxed)
            {
                info!("[微澜守护进程] 收到退出信号或托盘退出请求，服务安全退场。");
                return Ok(());
            }
            log::error!(
                "[微澜守护进程 错误] Wayland 输入法事件循环异常退出或无法启动: {}，准备终止进程并释放资源。",
                e
            );
            return Err(e);
        }
    }

    Ok(())
}
