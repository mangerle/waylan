//! 微澜 (WayLan) 核心数据类型、输入法状态与配置系统

pub mod candidate;
pub mod config;
pub mod constants;
pub mod input;
pub mod state;
pub mod text;

pub use candidate::{
    CIRCLED_NUMBERS, Candidate, CandidateBoxState, CandidateBoxView, CandidateExpansion,
    CandidateTier, CandidateViewMode, InputMode, TextAction, VoiceBoxState,
    format_candidate_display, format_candidate_index,
};
pub use config::{
    AppConfig, AudioFrontEndConfig, CandidateIndexStyle, CandidateOrientation, CloudAsrConfig,
    CloudProfile, CloudProviderType, ConfigError, FullWidthCharShortcut, FullWidthPunctShortcut,
    FuzzyPinyinRules, KeyModifiers, NsEngineType, OfflineAsrConfig, OfflineModelType, PageKeys,
    ParsedShortcut, SwitchKey, ThemeMode, TraditionalShortcut, TrayDisplayMode,
    TypoCorrectionRules, VadConfig, VadModelType, VoiceConfig, VoiceEngineType,
    VoiceInteractionConfig, VoiceItnConfig, VoiceVisualEffect, format_key_combination,
};
pub use constants::{
    MAX_CONFIG_FILE_BYTES, MAX_CONVERT_DICT_BYTES, MAX_IMPORT_FILE_BYTES, MAX_PHRASE_FILE_BYTES,
    MAX_USER_DICT_BYTES,
};
pub use input::*;
pub use state::{InputStateSnapshot, SharedInputFlags};
pub use text::pangu::{PanguSpacing, apply_pangu_spacing, should_insert_pangu_space};
pub use text::{is_cjk_character, to_full_width_char, to_full_width_string};
