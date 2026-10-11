//! 输入法守护进程词库加载、环境探测与会话配置热重载管理

use std::time::Instant;

use log::{info, warn};
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};
use waylan_algo::table::LexiconTable;
use waylan_algo::types::{AlgoConfig, FuzzyConfig};
use waylan_ui::UiTheme;
use waylan_wayland::probe_wayland_environment;
use waylan_wayland::session_v1::PageKeyConfig;

use crate::config::{AppConfig, PageKeys, SwitchKey, ThemeMode};

/// 按照多级搜索策略加载字词库
pub fn load_lexicon_table() -> LexiconTable {
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
pub fn load_auxiliary_dict() -> waylan_algo::AuxiliaryDict {
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

/// 按照多级搜索策略加载简繁转换两级词库（STPhrases + STCharacters）
pub fn load_opencc_dict() -> waylan_algo::OpenCcDict {
    info!("[简繁词库] 正在定位并加载简繁转换两级词库索引...");
    let search_paths = AppConfig::opencc_dict_search_paths();

    for path in &search_paths {
        if path.exists() {
            let load_start = Instant::now();
            match waylan_algo::OpenCcDict::load_from_file(path) {
                Ok(dict) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "  - 成功加载简繁转换词库: {:?} (耗时: {:.2} ms, 单字映射: {} 个, 消歧短语: {} 组)",
                        path,
                        elapsed.as_secs_f64() * 1000.0,
                        dict.chars.len(),
                        dict.phrases_len()
                    );
                    return dict;
                }
                Err(e) => {
                    warn!("  - 读取简繁词库文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    info!("  - 未检测到二进制简繁词库文件，回退加载开源 TSV 词表或内置核心表。");
    waylan_algo::OpenCcDict::load_default()
}

/// 统一会话配置包（收敛 V1 与 V2 热重载与初始化的重复配置逻辑）
pub struct SessionConfigBundle {
    pub algo_config: AlgoConfig,
    pub theme: UiTheme,
    pub font_size: f32,
    pub shift_mode: ToggleKeyMode,
    pub punct_processor: PunctuationProcessor,
    pub page_keys: PageKeyConfig,
    pub custom_phrases: waylan_algo::custom_phrase::CustomPhraseManager,
    pub user_dict_path: std::path::PathBuf,
    pub aux_dict: waylan_algo::AuxiliaryDict,
}

impl SessionConfigBundle {
    pub fn load_current() -> Self {
        let new_cfg = AppConfig::load_or_create();
        let algo_config = AlgoConfig {
            page_size: new_cfg.page_size,
            enable_v_mode: new_cfg.v_mode,
            traditional_chinese: new_cfg.traditional_chinese,
            fuzzy: if new_cfg.fuzzy_pinyin {
                FuzzyConfig::all_enabled()
            } else {
                FuzzyConfig::default()
            },
            auto_pangu_spacing: new_cfg.auto_pangu_spacing,
            ..Default::default()
        };
        let theme = match new_cfg.theme {
            ThemeMode::Light => UiTheme::light(),
            ThemeMode::Dark => UiTheme::dark(),
        };
        let shift_mode = match new_cfg.switch_key {
            SwitchKey::Shift => ToggleKeyMode::Shift,
            SwitchKey::Ctrl => ToggleKeyMode::Ctrl,
        };
        let punct_processor = PunctuationProcessor::with_options(
            new_cfg.slash_to_dunhao,
            new_cfg.protect_digits_punctuation,
            new_cfg.auto_complete_pairs,
        );
        let page_keys = match new_cfg.page_keys {
            PageKeys::MinusEqual => PageKeyConfig::MinusEqual,
            PageKeys::Bracket => PageKeyConfig::Bracket,
            PageKeys::CommaPeriod => PageKeyConfig::CommaPeriod,
        };
        let p_path = waylan_algo::custom_phrase::CustomPhraseManager::default_path();
        let custom_phrases = waylan_algo::custom_phrase::CustomPhraseManager::load_or_init(&p_path);
        let user_dict_path = AppConfig::user_dict_path();
        let aux_dict = load_auxiliary_dict();
        Self {
            algo_config,
            theme,
            font_size: new_cfg.font_size,
            shift_mode,
            punct_processor,
            page_keys,
            custom_phrases,
            user_dict_path,
            aux_dict,
        }
    }
}

/// 执行 V2 会话的配置与词库热重载
pub fn apply_reload_v2(s: &mut waylan_wayland::session_v2::WaylanV2Session) {
    let b = SessionConfigBundle::load_current();
    s.engine.config = b.algo_config;
    s.surface_manager.renderer.theme = b.theme;
    s.surface_manager.renderer.font_size = b.font_size;
    s.shift_state = ShiftToggleStateMachine::with_toggle_key(b.shift_mode);
    s.punctuation_processor = b.punct_processor;
    s.page_keys = b.page_keys;
    s.engine.custom_phrases = b.custom_phrases;
    s.engine.set_user_dict_path(&b.user_dict_path);
    s.engine.load_auxiliary_dict(b.aux_dict);
    if !s.engine.raw_input().is_empty() {
        s.refresh_candidate_surface();
    }
}

/// 执行 V1 会话的配置与词库热重载
pub fn apply_reload_v1(s: &mut waylan_wayland::session_v1::WaylanV1Session) {
    let b = SessionConfigBundle::load_current();
    s.engine.config = b.algo_config;
    s.surface_manager.renderer.theme = b.theme;
    s.surface_manager.renderer.font_size = b.font_size;
    s.shift_sm = ShiftToggleStateMachine::with_toggle_key(b.shift_mode);
    s.punctuation_processor = b.punct_processor;
    s.page_keys = b.page_keys;
    s.engine.custom_phrases = b.custom_phrases;
    s.engine.set_user_dict_path(&b.user_dict_path);
    s.engine.load_auxiliary_dict(b.aux_dict);
    if !s.engine.raw_input().is_empty() {
        s.refresh_candidate_surface();
    }
}

/// 检查 Wayland 合成器协议能力
pub fn check_wayland_capabilities() -> waylan_wayland::WaylandCapabilities {
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
            caps
        }
        Err(e) => {
            warn!(
                "[系统环境 警告] 无法直接连接默认 Wayland 会话 ({})，请确保 WAYLAND_DISPLAY 环境变量已设置。",
                e
            );
            waylan_wayland::WaylandCapabilities::default()
        }
    }
}
