mod config;
mod logger;
mod settings_gui;
mod tray;

use config::{AppConfig, ThemeMode};
use log::{info, warn};
use std::env;
use std::path::Path;
use std::time::Instant;
use waylan_algo::builder::DictBuilder;
use waylan_algo::table::LexiconTable;
use waylan_algo::types::AlgoConfig;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AlgoEngine;
use waylan_ui::{CandidateBoxRenderer, UiTheme};
use waylan_wayland::probe_wayland_environment;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    // 0. 处理版本查询参数
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V" || args[1] == "-v") {
        println!("waylan {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // 1. 处理 settings 子命令（图形化偏好设置界面）
    if args.len() >= 2 && (args[1] == "settings" || args[1] == "--settings") {
        return settings_gui::run_settings_gui();
    }

    // 2. 处理 dict 子命令（支持词库打包与用户自定义词库导入）
    if args.len() >= 2 && args[1] == "dict" {
        return handle_dict_subcommand(&args[2..]);
    }

    // 2. 初始化双通道日志系统（终端输出 + 文件落盘）
    match logger::init_logger() {
        Ok(path) => {
            info!("微澜 (WayLan) 日志系统初始化完成，持久化文件: {:?}", path);
        }
        Err(e) => {
            eprintln!(
                "[日志警告] 初始化文件日志失败 ({})，将仅输出到标准错误。",
                e
            );
        }
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
    let algo_config = AlgoConfig {
        page_size: cfg.page_size,
        enable_v_mode: cfg.v_mode,
        traditional_chinese: cfg.traditional_chinese,
        double_pinyin: cfg.double_pinyin,
        auto_pangu_spacing: cfg.auto_pangu_spacing,
        ..Default::default()
    };
    let mut engine = AlgoEngine::new(table, algo_config);

    // 加载自定义短语与用户历史自造词库
    let phrase_path = waylan_algo::custom_phrase::CustomPhraseManager::default_path();
    engine.custom_phrases = waylan_algo::custom_phrase::CustomPhraseManager::load_or_init(&phrase_path);
    let user_dict = AppConfig::user_dict_path();
    if user_dict.exists() {
        if let Ok(model) = waylan_algo::user_adaptive::UserAdaptiveModel::load_from_file(&user_dict) {
            info!("[字库引擎] 成功载入用户自适应个性化历史词库: {:?}", user_dict);
            engine.user_model = model;
        }
    }

    // 载入混输与辅助符号词库 (aux.dict.bin)
    let aux_dict = load_auxiliary_dict();
    engine.load_auxiliary_dict(aux_dict);

    // 6. 探测 Wayland 环境与合成器协议能力
    check_wayland_capabilities();

    // 7. 执行环境自检（--check 参数）
    if args.len() > 1 && args[1] == "--check" {
        run_self_check(&mut engine)?;
        return Ok(());
    }

    // 8. 启动系统任务栏托盘常驻服务 (StatusNotifierItem)
    let _tray = tray::TrayService::spawn();

    info!("------------------------------------------------------------");
    info!("[微澜守护进程] 核心服务启动就绪，正在后台监听键盘事件。");
    info!("------------------------------------------------------------");

    // 保持主服务持续在后台常驻运行
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// 按照多级搜索策略加载字词库
fn load_lexicon_table() -> LexiconTable {
    info!("[字库引擎] 正在定位并加载字词库索引...");
    let search_paths = AppConfig::base_dict_search_paths();

    for path in &search_paths {
        if path.exists() {
            let load_start = Instant::now();
            match LexiconTable::load_binary(path) {
                Ok(loaded) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "  - 成功加载全量汉语词库: {:?} (耗时: {:.2} ms, 拼音键数: {} 组)",
                        path,
                        elapsed.as_secs_f64() * 1000.0,
                        loaded.len()
                    );
                    return loaded;
                }
                Err(e) => {
                    warn!("  - 读取词库文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    warn!("  - 未检测到有效的基础外部词库文件，回退到内置基础测试词库。");
    LexiconTable::with_sample_data()
}

/// 按照多级搜索策略加载辅助与混输词库（英文与符号表情）
fn load_auxiliary_dict() -> waylan_algo::AuxiliaryDict {
    info!("[辅助词库] 正在定位并加载混输与符号词库索引...");
    let search_paths = AppConfig::aux_dict_search_paths();

    for path in &search_paths {
        if path.exists() {
            let load_start = Instant::now();
            match waylan_algo::AuxiliaryDict::load_from_file(path) {
                Ok(dict) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "  - 成功加载辅助混输词库: {:?} (耗时: {:.2} ms, 英文词数: {}, 符号表情: {} 组)",
                        path,
                        elapsed.as_secs_f64() * 1000.0,
                        dict.english_words.len(),
                        dict.symbols.len()
                    );
                    return dict;
                }
                Err(e) => {
                    warn!("  - 读取辅助词库文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    info!("  - 未检测到外部辅助词库文件，使用系统内置保底英文与符号表情库。");
    waylan_algo::AuxiliaryDict::with_default_data()
}

/// 检查 Wayland 合成器协议能力
fn check_wayland_capabilities() {
    info!("[系统环境] 正在检查当前 Wayland 会话与合成器能力...");
    match probe_wayland_environment() {
        Ok(caps) => {
            info!(
                "[系统环境] 成功连接到 Wayland 会话：wl_compositor={}, wl_seat={}, zwp_input_method_v1={}, zwp_input_method_manager_v2={}, zwlr_layer_shell_v1={}",
                caps.has_compositor,
                caps.has_seat,
                caps.has_input_method_v1,
                caps.has_input_method_v2,
                caps.has_layer_shell_v1
            );
        }
        Err(e) => {
            warn!(
                "[系统环境 警告] 无法直接连接默认 Wayland 会话 ({})，请确保 WAYLAND_DISPLAY 环境变量已设置。",
                e
            );
        }
    }
}

/// 处理 dict 命令行子命令（支持 build 和 import）
fn handle_dict_subcommand(sub_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if sub_args.is_empty() {
        print_dict_usage();
        return Ok(());
    }

    match sub_args[0].as_str() {
        "--help" | "-h" => {
            print_dict_usage();
        }
        "build" => {
            if sub_args.len() < 3 {
                print_dict_usage();
                return Ok(());
            }
            let output_path = &sub_args[sub_args.len() - 1];
            let input_paths = &sub_args[1..sub_args.len() - 1];
            println!("[词库构建] 正在将文本词库打包为二进制: {}", output_path);
            let stats = DictBuilder::build_binary_dict(input_paths, Path::new(output_path))?;
            println!(
                "[构建完成] 行数: {}, 有效词条: {}, 键数: {}, 耗时: {} ms",
                stats.total_lines, stats.total_words, stats.total_keys, stats.elapsed_ms
            );
        }
        "import" => {
            if sub_args.len() < 2 {
                print_dict_usage();
                return Ok(());
            }
            let import_file = &sub_args[1];
            println!("[词库导入] 正在导入用户自定义词库: {}", import_file);
            let user_dict_path = AppConfig::user_dict_path();
            let mut user_model = UserAdaptiveModel::load_from_file(&user_dict_path)
                .unwrap_or_else(|_| UserAdaptiveModel::new());

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let count =
                DictBuilder::import_user_text_dict(Path::new(import_file), &mut user_model, now)?;
            user_model.save_to_file(&user_dict_path)?;
            println!("[导入成功] 已将 {} 条自定义词条注入用户自适应词库！", count);
        }
        other => {
            println!("未知的 dict 子命令: {}", other);
        }
    }
    Ok(())
}

fn print_dict_usage() {
    println!("用法:");
    println!("  waylan dict build <输入文本1> [输入文本2...] <输出文件.dict.bin>");
    println!("  waylan dict import <用户自定义词库.txt>");
}

/// 执行自检测试
fn run_self_check(engine: &mut AlgoEngine) -> Result<(), Box<dyn std::error::Error>> {
    info!("------------------------------------------------------------");
    info!("[自检] 正在测试拼音检索性能与首选项准确度...");
    let test_queries = [
        "xian", "women", "zhongguo", "weilan", "v1+2*3", "uhehe", "riqi", "shijian",
    ];
    for q in test_queries {
        engine.reset();
        let query_start = Instant::now();
        for ch in q.chars() {
            engine.push_char(ch);
        }
        let query_time = query_start.elapsed();
        let cands = engine.candidates();
        let formatted = engine.formatted_preedit();
        let top7: Vec<String> = cands
            .iter()
            .take(7)
            .map(|c| {
                if c.is_expandable {
                    format!("{}[扩展:{}项]", c.text, c.sub_candidates.len())
                } else {
                    c.text.clone()
                }
            })
            .collect();
        info!(
            "  - 输入: {:<10} (切分: {:<10}) => 候选 ({}项): {} (耗时: {:.2} µs)",
            q,
            formatted,
            top7.len(),
            top7.join(" / "),
            query_time.as_secs_f64() * 1_000_000.0
        );
    }
    info!("[自检完成] 微澜输入法环境、全量字词库与核心算法模块全部就绪。");
    Ok(())
}
