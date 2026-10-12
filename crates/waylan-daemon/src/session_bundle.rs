//! 输入法守护进程词库加载、环境探测与会话配置热重载管理

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use log::{info, warn};
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};
use waylan_algo::table::LexiconTable;
use waylan_algo::types::{AlgoConfig, FuzzyConfig};
use waylan_algo::user_adaptive::UserAdaptiveModel;
#[cfg(test)]
use waylan_algo::OpenCcDict;
use waylan_algo::{AuxiliaryDict, EnglishSuggestionManager, SymbolSuggestionManager};
use waylan_ui::UiTheme;
use waylan_wayland::probe_wayland_environment;
use waylan_wayland::session_v1::{PageKeyConfig, WaylanV1Session};
use waylan_wayland::session_v2::WaylanV2Session;
use waylan_wayland::surface::CandidateSurfaceManager;
use waylan_wayland::WaylandCapabilities;

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

/// 按照多级搜索策略加载辅助与混输词库（英文与符号表情，供单元测试使用）
#[cfg(test)]
pub fn load_auxiliary_dict() -> AuxiliaryDict {
    info!("[辅助词库] 正在定位并加载混输与符号词库索引...");
    let search_paths = AppConfig::aux_dict_search_paths();

    for path in &search_paths {
        if path.exists() {
            let load_start = Instant::now();
            match AuxiliaryDict::load_from_file(path) {
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
    AuxiliaryDict::with_default_data()
}

static CACHED_AUX_MANAGERS: OnceLock<(
    Arc<EnglishSuggestionManager>,
    Arc<SymbolSuggestionManager>,
)> = OnceLock::new();

/// 按照多级搜索策略加载辅助与混输词库管理器（单例缓存，彻底消除热重载时重复解析与 6 万次 String 堆分配）
pub fn load_auxiliary_managers() -> (Arc<EnglishSuggestionManager>, Arc<SymbolSuggestionManager>) {
    CACHED_AUX_MANAGERS
        .get_or_init(|| {
            info!("[辅助词库] 正在定位并初始化辅助与混输词典索引...");
            let search_paths = AppConfig::aux_dict_search_paths();

            for path in &search_paths {
                if path.exists() {
                    let load_start = Instant::now();
                    match AuxiliaryDict::load_from_file(path) {
                        Ok(dict) => {
                            let elapsed = load_start.elapsed();
                            info!(
                                "  - 成功加载辅助混输词库: {:?} (耗时: {:.2} ms, 英文词数: {}, 符号表情: {} 组)",
                                path,
                                elapsed.as_secs_f64() * 1000.0,
                                dict.english_words.len(),
                                dict.symbols.len()
                            );
                            let en =
                                Arc::new(EnglishSuggestionManager::from_words(dict.english_words));
                            let sym =
                                Arc::new(SymbolSuggestionManager::from_symbols(dict.symbols));
                            return (en, sym);
                        }
                        Err(e) => {
                            warn!("  - 读取辅助词库文件 {:?} 失败: {}", path, e);
                        }
                    }
                }
            }

            info!("  - 未检测到外部辅助词库文件，使用系统内置单例保底英文与符号表情库。");
            (
                EnglishSuggestionManager::default_global(),
                SymbolSuggestionManager::default_global(),
            )
        })
        .clone()
}

/// 按照多级搜索策略加载简繁转换两级词库（复用 OpenCcDict 全局单例缓存，供单元测试使用）
#[cfg(test)]
pub fn load_opencc_dict() -> std::sync::Arc<OpenCcDict> {
    OpenCcDict::global()
}

/// 统一会话配置包（收敛 V1 与 V2 热重载与初始化的重复配置逻辑）
pub struct SessionConfigBundle {
    pub algo_config: AlgoConfig,
    pub theme: UiTheme,
    pub font_size: f32,
    pub shift_mode: ToggleKeyMode,
    pub punct_processor: PunctuationProcessor,
    pub page_keys: PageKeyConfig,
    pub custom_phrases: CustomPhraseManager,
    pub user_dict_path: std::path::PathBuf,
    pub english_suggester: Arc<EnglishSuggestionManager>,
    pub symbol_suggester: Arc<SymbolSuggestionManager>,
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
        let p_path = CustomPhraseManager::default_path();
        let custom_phrases = CustomPhraseManager::load_or_init(&p_path);
        let user_dict_path = AppConfig::user_dict_path();
        let (english_suggester, symbol_suggester) = load_auxiliary_managers();
        Self {
            algo_config,
            theme,
            font_size: new_cfg.font_size,
            shift_mode,
            punct_processor,
            page_keys,
            custom_phrases,
            user_dict_path,
            english_suggester,
            symbol_suggester,
        }
    }

    /// 将配置包原子应用到输入引擎与候选框表面管理器中
    pub fn apply_to_session_core(
        self,
        engine: &mut waylan_algo::AlgoEngine,
        surface_manager: &mut CandidateSurfaceManager,
        punct_processor: &mut PunctuationProcessor,
        page_keys: &mut PageKeyConfig,
    ) {
        engine.config = self.algo_config;
        surface_manager.renderer.theme = self.theme;
        surface_manager.renderer.font_size = self.font_size;
        *punct_processor = self.punct_processor;
        *page_keys = self.page_keys;
        engine.custom_phrases = self.custom_phrases;
        if self.user_dict_path.exists() {
            if let Ok(m) = std::fs::metadata(&self.user_dict_path) {
                if m.len() == 0 {
                    info!("[热重载] 检测到用户词库文件为空，重置内存用户自适应模型");
                    engine.user_model = UserAdaptiveModel::new();
                    engine.set_user_dict_path(&self.user_dict_path);
                } else if engine.user_dict_path.as_deref() != Some(&self.user_dict_path) {
                    engine.set_user_dict_path(&self.user_dict_path);
                }
            }
        } else {
            info!("[热重载] 检测到用户词库文件已被删除或清空，重置内存用户自适应模型");
            engine.user_model = UserAdaptiveModel::new();
            engine.set_user_dict_path(&self.user_dict_path);
        }
        engine.english_suggester = self.english_suggester;
        engine.symbol_suggester = self.symbol_suggester;
    }
}

/// 执行 V2 会话的配置与词库热重载
pub fn apply_reload_v2(s: &mut WaylanV2Session) {
    let b = SessionConfigBundle::load_current();
    s.shift_state = ShiftToggleStateMachine::with_toggle_key(b.shift_mode);
    b.apply_to_session_core(
        &mut s.engine,
        &mut s.surface_manager,
        &mut s.punctuation_processor,
        &mut s.page_keys,
    );
    if !s.engine.raw_input().is_empty() {
        s.refresh_candidate_surface();
    }
}

/// 执行 V1 会话的配置与词库热重载
pub fn apply_reload_v1(s: &mut WaylanV1Session) {
    let b = SessionConfigBundle::load_current();
    s.shift_sm = ShiftToggleStateMachine::with_toggle_key(b.shift_mode);
    b.apply_to_session_core(
        &mut s.engine,
        &mut s.surface_manager,
        &mut s.punctuation_processor,
        &mut s.page_keys,
    );
    if !s.engine.raw_input().is_empty() {
        s.refresh_candidate_surface();
    }
}

/// 检查 Wayland 合成器协议能力
pub fn check_wayland_capabilities() -> WaylandCapabilities {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_config_bundle_load() {
        let bundle = SessionConfigBundle::load_current();
        assert!(bundle.algo_config.page_size >= 5 && bundle.algo_config.page_size <= 9);
        assert!(bundle.font_size >= 12.0 && bundle.font_size <= 24.0);
    }

    #[test]
    fn test_load_lexicon_table_fallback() {
        let table = load_lexicon_table();
        assert!(!table.is_empty(), "回退词库应包含基础词条数据");
    }

    #[test]
    fn test_load_auxiliary_and_opencc_dict_fallback() {
        let aux = load_auxiliary_dict();
        assert!(!aux.english_words.is_empty(), "辅助词库应包含基础英文词条");
        assert!(!aux.symbols.is_empty(), "辅助词库应包含基础符号映射");

        let opencc = load_opencc_dict();
        assert!(!opencc.chars.is_empty(), "简繁词库应包含基础字符映射");
    }
}
