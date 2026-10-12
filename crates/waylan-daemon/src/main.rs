mod cli;
mod config;
mod logger;
mod session_bundle;
mod settings_gui;
mod tray;

use config::{AppConfig, PageKeys, SwitchKey, ThemeMode};
use log::{info, warn};
use session_bundle::{
    apply_reload_v1, apply_reload_v2, check_wayland_capabilities, load_auxiliary_dict,
    load_lexicon_table, load_opencc_dict,
};
use std::env;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};
use waylan_algo::types::{AlgoConfig, FuzzyConfig};
use waylan_algo::AlgoEngine;
use waylan_ui::{CandidateBoxRenderer, UiTheme};
use waylan_wayland::session_v1::PageKeyConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    // 0. 处理版本查询参数
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V" || args[1] == "-v") {
        println!("waylan {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // 1. 初始化双通道日志系统（终端输出 + 文件落盘），确保后续 CLI 子命令、设置 GUI 与主服务日志正常输出
    let log_init_res = logger::init_logger();
    match log_init_res {
        Ok(ref path) => {
            info!("微澜 (WayLan) 日志系统初始化完成，持久化文件: {:?}", path);
        }
        Err(ref e) => {
            eprintln!("[微澜] 日志系统初始化失败: {}", e);
        }
    }

    // 2. 处理 settings 子命令（图形化偏好设置界面）
    if args.len() >= 2 && (args[1] == "settings" || args[1] == "--settings") {
        return settings_gui::run_settings_gui();
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

    // 处理自检参数或未知参数检查（排除合法的无参启动或合法选项）
    if args.len() >= 2 && args[1] != "--check" {
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
    let aux_dict = load_auxiliary_dict();
    engine.load_auxiliary_dict(aux_dict);

    // 载入简繁转换两级词库 (opencc.dict.bin)
    let opencc_dict = load_opencc_dict();
    engine.load_opencc_dict(opencc_dict);

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

    // 8. 启动事件循环跨线程唤醒通道与系统任务栏托盘常驻服务 (StatusNotifierItem)
    let loop_wakeup = waylan_wayland::LoopWakeup::new().ok();
    let tray = tray::TrayService::spawn(std::sync::Arc::clone(&term_now), loop_wakeup.clone());

    info!("------------------------------------------------------------");
    info!("[微澜守护进程] 核心服务启动就绪，正在准备建立 Wayland 输入法会话。");
    info!("------------------------------------------------------------");

    let mut watched_paths = Vec::with_capacity(4);
    if let Some(c_path) = AppConfig::config_path() {
        watched_paths.push(c_path);
    }
    watched_paths.push(CustomPhraseManager::default_path());
    watched_paths.push(AppConfig::user_aux_dict_path());
    watched_paths.push(AppConfig::user_dict_path());

    let initial_mtime = watched_paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().and_then(|m| m.modified().ok()))
        .max();

    // 9. 构造并驱动 Wayland 输入法会话事件循环（优先协商 V2 协议，不支持时自动回退 V1 协议）
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
        session_v2.punctuation_processor = PunctuationProcessor::with_options(
            cfg.slash_to_dunhao,
            cfg.protect_digits_punctuation,
            cfg.auto_complete_pairs,
        );
        session_v2.page_keys = match cfg.page_keys {
            PageKeys::MinusEqual => PageKeyConfig::MinusEqual,
            PageKeys::Bracket => PageKeyConfig::Bracket,
            PageKeys::CommaPeriod => PageKeyConfig::CommaPeriod,
        };
        session_v2.shared_is_chinese = Some(tray.mode_flag());
        session_v2.mode_notify_tx = Some(tray.icon_notifier());
        session_v2.reload_requested = Some(tray.reload_flag());
        session_v2.exit_requested = Some(tray.exit_flag());
        session_v2.loop_wakeup = loop_wakeup.clone();
        session_v2.reload_callback = Some(Box::new(apply_reload_v2));
        let v2_result = waylan_wayland::run_session_v2_loop_fallible(session_v2);
        match v2_result {
            Ok(()) => Ok(()),
            Err((e, returned_session)) => {
                if tray.is_exit_requested() {
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
                session.shared_is_chinese = Some(tray.mode_flag());
                session.mode_notify_tx = Some(tray.icon_notifier());
                session.reload_requested = Some(tray.reload_flag());
                session.exit_requested = Some(tray.exit_flag());
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
        session.page_keys = match cfg.page_keys {
            PageKeys::MinusEqual => PageKeyConfig::MinusEqual,
            PageKeys::Bracket => PageKeyConfig::Bracket,
            PageKeys::CommaPeriod => PageKeyConfig::CommaPeriod,
        };
        session.shared_is_chinese = Some(tray.mode_flag());
        session.mode_notify_tx = Some(tray.icon_notifier());
        session.reload_requested = Some(tray.reload_flag());
        session.exit_requested = Some(tray.exit_flag());
        session.loop_wakeup = loop_wakeup;
        session.reload_callback = Some(Box::new(apply_reload_v1));
        waylan_wayland::run_session_v1_loop(session)
    };

    match loop_result {
        Ok(_) => {
            info!("[微澜守护进程] Wayland 输入法事件循环已正常退出，所有资源已安全释放。");
        }
        Err(e) => {
            if tray.is_exit_requested() || term_now.load(std::sync::atomic::Ordering::Relaxed) {
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
