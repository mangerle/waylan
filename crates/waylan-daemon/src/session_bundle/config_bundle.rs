//! 统一会话配置包与 V1/V2 热重载驱动

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use log::info;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};
use waylan_algo::types::{AlgoConfig, FuzzyConfig};
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::{AlgoEngine, EnglishSuggestionManager, SymbolSuggestionManager};
use waylan_ui::UiTheme;
use waylan_wayland::session_v1::{PageKeyConfig, WaylanV1Session};
use waylan_wayland::session_v2::WaylanV2Session;

use super::dict_loader::load_auxiliary_managers;
use crate::config::{
    AppConfig, CandidateIndexStyle, CandidateOrientation, FullWidthCharShortcut,
    FullWidthPunctShortcut, SwitchKey, ThemeMode, TraditionalShortcut, VoiceConfig,
};

/// 统一会话配置包（收敛 V1、V2 与 IBus 热重载与初始化的重复配置逻辑）
pub struct SessionConfigBundle {
    pub algo_config: AlgoConfig,
    pub theme: UiTheme,
    pub font_size: f32,
    pub shift_mode: ToggleKeyMode,
    pub punct_processor: PunctuationProcessor,
    pub page_keys: PageKeyConfig,
    pub custom_phrases: CustomPhraseManager,
    pub user_dict_path: PathBuf,
    pub english_suggester: Arc<EnglishSuggestionManager>,
    pub symbol_suggester: Arc<SymbolSuggestionManager>,
    pub exclusive_input_source: bool,
    pub candidate_index_style: CandidateIndexStyle,
    pub candidate_orientation: CandidateOrientation,
    pub traditional_shortcut: TraditionalShortcut,
    pub full_width_char_shortcut: FullWidthCharShortcut,
    pub full_width_punct_shortcut: FullWidthPunctShortcut,
    pub voice: VoiceConfig,
}

impl SessionConfigBundle {
    pub fn load_current() -> Self {
        let new_cfg = AppConfig::load_or_create();
        let algo_config = AlgoConfig {
            page_size: new_cfg.page_size,
            enable_v_mode: new_cfg.v_mode,
            enable_u_mode: new_cfg.enable_u_mode,
            enable_i_mode: new_cfg.enable_i_mode,
            traditional_chinese: new_cfg.traditional_chinese,
            filter_uncommon_charset: new_cfg.filter_uncommon_charset,
            show_pinyin_comment: new_cfg.show_pinyin_comment,
            auto_dedup: new_cfg.auto_dedup,
            fuzzy: if new_cfg.fuzzy_pinyin {
                FuzzyConfig {
                    z_zh: new_cfg.fuzzy_rules.z_zh,
                    c_ch: new_cfg.fuzzy_rules.c_ch,
                    s_sh: new_cfg.fuzzy_rules.s_sh,
                    an_ang: new_cfg.fuzzy_rules.an_ang,
                    en_eng: new_cfg.fuzzy_rules.en_eng,
                    in_ing: new_cfg.fuzzy_rules.in_ing,
                    l_n: new_cfg.fuzzy_rules.l_n,
                    f_h: new_cfg.fuzzy_rules.f_h,
                    r_l: new_cfg.fuzzy_rules.r_l,
                    g_k: new_cfg.fuzzy_rules.g_k,
                }
            } else {
                FuzzyConfig::default()
            },
            auto_pangu_spacing: new_cfg.auto_pangu_spacing,
        };
        let theme = match new_cfg.theme {
            ThemeMode::Light => UiTheme::light(),
            ThemeMode::Dark => UiTheme::dark(),
        };
        let shift_mode = match new_cfg.switch_key {
            SwitchKey::Shift => ToggleKeyMode::Shift,
            SwitchKey::Ctrl => ToggleKeyMode::Ctrl,
        };
        let mut punct_processor = PunctuationProcessor::with_options(
            new_cfg.slash_to_dunhao,
            new_cfg.protect_digits_punctuation,
            new_cfg.auto_complete_pairs,
        );
        punct_processor.full_width_punct = new_cfg.full_width_punct;
        let page_keys = PageKeyConfig::from(new_cfg.page_keys);
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
            exclusive_input_source: new_cfg.exclusive_input_source,
            candidate_index_style: new_cfg.candidate_index_style,
            candidate_orientation: new_cfg.candidate_orientation,
            traditional_shortcut: new_cfg.traditional_shortcut,
            full_width_char_shortcut: new_cfg.full_width_char_shortcut,
            full_width_punct_shortcut: new_cfg.full_width_punct_shortcut,
            voice: new_cfg.voice,
        }
    }

    /// 将配置包原子应用到算法引擎与按键/标点状态机中（完整保留标点上下文与未变更的按键跟踪状态）
    pub fn apply_to_engine_and_input(
        self,
        engine: &mut AlgoEngine,
        shift_sm: &mut ShiftToggleStateMachine,
        punct_processor: &mut PunctuationProcessor,
        page_keys: &mut PageKeyConfig,
    ) {
        engine.config = self.algo_config;
        shift_sm.set_toggle_key(self.shift_mode);
        punct_processor.apply_options_from(&self.punct_processor);
        *page_keys = self.page_keys;
        engine.custom_phrases = self.custom_phrases;
        if self.user_dict_path.exists() {
            if let Ok(m) = fs::metadata(&self.user_dict_path) {
                if m.len() == 0 {
                    info!("[热重载] 检测到用户词库文件为空，重置内存用户自适应模型");
                    engine.user_model = UserAdaptiveModel::new();
                    engine.set_user_dict_path(&self.user_dict_path);
                } else if !engine.user_model.is_same_wal_file(&self.user_dict_path) {
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
    s.surface_manager.renderer.theme = b.theme.clone();
    s.surface_manager.renderer.font_size = b.font_size;
    b.apply_to_engine_and_input(
        &mut s.engine,
        &mut s.shift_state,
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
    s.surface_manager.renderer.theme = b.theme.clone();
    s.surface_manager.renderer.font_size = b.font_size;
    b.apply_to_engine_and_input(
        &mut s.engine,
        &mut s.shift_sm,
        &mut s.punctuation_processor,
        &mut s.page_keys,
    );
    if !s.engine.raw_input().is_empty() {
        s.refresh_candidate_surface();
    }
}
