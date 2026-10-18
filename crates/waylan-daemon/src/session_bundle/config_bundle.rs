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

/// 根据应用程序配置构建算法引擎配置（冷启动与热重载共用的单一真实源）
pub fn build_algo_config(cfg: &AppConfig) -> AlgoConfig {
    AlgoConfig {
        page_size: cfg.ui.page_size,
        enable_v_mode: cfg.pinyin.v_mode,
        enable_u_mode: cfg.pinyin.enable_u_mode,
        enable_i_mode: cfg.pinyin.enable_i_mode,
        traditional_chinese: cfg.pinyin.traditional_chinese,
        filter_uncommon_charset: cfg.pinyin.filter_uncommon_charset,
        show_pinyin_comment: cfg.ui.show_pinyin_comment,
        auto_dedup: cfg.pinyin.auto_dedup,
        fuzzy: if cfg.pinyin.fuzzy_pinyin {
            FuzzyConfig {
                z_zh: cfg.pinyin.fuzzy_rules.z_zh,
                c_ch: cfg.pinyin.fuzzy_rules.c_ch,
                s_sh: cfg.pinyin.fuzzy_rules.s_sh,
                an_ang: cfg.pinyin.fuzzy_rules.an_ang,
                en_eng: cfg.pinyin.fuzzy_rules.en_eng,
                in_ing: cfg.pinyin.fuzzy_rules.in_ing,
                l_n: cfg.pinyin.fuzzy_rules.l_n,
                f_h: cfg.pinyin.fuzzy_rules.f_h,
                r_l: cfg.pinyin.fuzzy_rules.r_l,
                g_k: cfg.pinyin.fuzzy_rules.g_k,
            }
        } else {
            FuzzyConfig::default()
        },
        auto_pangu_spacing: cfg.pinyin.auto_pangu_spacing,
    }
}

impl SessionConfigBundle {
    pub fn load_current() -> Self {
        let new_cfg = AppConfig::load_or_create();
        let algo_config = build_algo_config(&new_cfg);
        let theme = match new_cfg.ui.theme {
            ThemeMode::Light => UiTheme::light(),
            ThemeMode::Dark => UiTheme::dark(),
        };
        let shift_mode = match new_cfg.shortcuts.switch_key {
            SwitchKey::Shift => ToggleKeyMode::Shift,
            SwitchKey::Ctrl => ToggleKeyMode::Ctrl,
        };
        let mut punct_processor = PunctuationProcessor::with_options(
            new_cfg.pinyin.slash_to_dunhao,
            new_cfg.pinyin.protect_digits_punctuation,
            new_cfg.pinyin.auto_complete_pairs,
        );
        punct_processor.full_width_punct = new_cfg.pinyin.full_width_punct;
        let page_keys = PageKeyConfig::from(new_cfg.shortcuts.page_keys);
        let p_path = CustomPhraseManager::default_path();
        let custom_phrases = CustomPhraseManager::load_or_init(&p_path);
        let user_dict_path = AppConfig::user_dict_path();
        let (english_suggester, symbol_suggester) = load_auxiliary_managers();
        Self {
            algo_config,
            theme,
            font_size: new_cfg.ui.font_size,
            shift_mode,
            punct_processor,
            page_keys,
            custom_phrases,
            user_dict_path,
            english_suggester,
            symbol_suggester,
            exclusive_input_source: new_cfg.desktop.exclusive_input_source,
            candidate_index_style: new_cfg.ui.candidate_index_style,
            candidate_orientation: new_cfg.ui.candidate_orientation,
            traditional_shortcut: new_cfg.shortcuts.traditional_shortcut,
            full_width_char_shortcut: new_cfg.shortcuts.full_width_char_shortcut,
            full_width_punct_shortcut: new_cfg.shortcuts.full_width_punct_shortcut,
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
        engine.resources.custom_phrases = self.custom_phrases;
        if self.user_dict_path.exists() {
            if let Ok(m) = fs::metadata(&self.user_dict_path) {
                if m.len() == 0 {
                    info!("[热重载] 检测到用户词库文件为空，重置内存用户自适应模型");
                    engine.adaptive.model = UserAdaptiveModel::new();
                    engine.set_user_dict_path(&self.user_dict_path);
                } else if !engine.is_same_wal_file(&self.user_dict_path) {
                    engine.set_user_dict_path(&self.user_dict_path);
                }
            }
        } else {
            info!("[热重载] 检测到用户词库文件已被删除或清空，重置内存用户自适应模型");
            engine.adaptive.model = UserAdaptiveModel::new();
            engine.set_user_dict_path(&self.user_dict_path);
        }
        engine.resources.english_suggester = self.english_suggester;
        engine.resources.symbol_suggester = self.symbol_suggester;
    }

    /// 将配置包原子应用到输入法核心会话状态中
    pub fn apply_to_core(self, core: &mut waylan_algo::ImeCoreSession) {
        self.apply_to_engine_and_input(
            &mut core.engine,
            &mut core.shift_sm,
            &mut core.punctuation_processor,
            &mut core.page_keys,
        );
    }
}

/// 支持主题与核心算法热重载的 Wayland 会话抽象
pub trait ReloadableWaylandSession {
    /// 获取候选表面渲染器的可变引用
    fn renderer_mut(&mut self) -> &mut waylan_ui::CandidateBoxRenderer;
    /// 获取核心输入会话的可变引用
    fn core_mut(&mut self) -> &mut waylan_algo::ImeCoreSession;
    /// 当拼音输入非空时刷新候选框表面
    fn refresh_surface_if_composing(&mut self);
}

impl ReloadableWaylandSession for WaylanV1Session {
    fn renderer_mut(&mut self) -> &mut waylan_ui::CandidateBoxRenderer {
        &mut self.surface_manager.renderer
    }

    fn core_mut(&mut self) -> &mut waylan_algo::ImeCoreSession {
        &mut self.core
    }

    fn refresh_surface_if_composing(&mut self) {
        if !self.core.engine.raw_input().is_empty() {
            self.refresh_candidate_surface();
        }
    }
}

impl ReloadableWaylandSession for WaylanV2Session {
    fn renderer_mut(&mut self) -> &mut waylan_ui::CandidateBoxRenderer {
        &mut self.surface_manager.renderer
    }

    fn core_mut(&mut self) -> &mut waylan_algo::ImeCoreSession {
        &mut self.core
    }

    fn refresh_surface_if_composing(&mut self) {
        if !self.core.engine.raw_input().is_empty() {
            self.refresh_candidate_surface();
        }
    }
}

/// 统一执行 Wayland 会话的配置与词库热重载（单一真相源）
pub fn apply_wayland_session_reload<S: ReloadableWaylandSession>(session: &mut S) {
    let bundle = SessionConfigBundle::load_current();
    session.renderer_mut().theme = bundle.theme.clone();
    session.renderer_mut().font_size = bundle.font_size;
    bundle.apply_to_core(session.core_mut());
    session.refresh_surface_if_composing();
}

/// 执行 V2 会话的配置与词库热重载
pub fn apply_reload_v2(s: &mut WaylanV2Session) {
    apply_wayland_session_reload(s);
}

/// 执行 V1 会话的配置与词库热重载
pub fn apply_reload_v1(s: &mut WaylanV1Session) {
    apply_wayland_session_reload(s);
}
