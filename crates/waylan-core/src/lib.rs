//! 微澜 (WayLan) 核心数据类型、输入法状态与配置系统

pub mod assets;
pub mod candidate;
pub mod config;
pub mod constants;
pub mod input;
pub mod state;
pub mod text;

pub use candidate::{
    Candidate, CandidateBoxState, ExpandedCardState, GridState, InputMode, TextAction,
    VoiceBoxState,
};
pub use config::{
    format_key_combination, AppConfig, AudioFrontEndConfig, CandidateIndexStyle,
    CandidateOrientation, CloudAsrConfig, CloudProfile, CloudProviderType, ConfigError,
    FullWidthCharShortcut, FullWidthPunctShortcut, FuzzyPinyinRules, NsEngineType,
    OfflineAsrConfig, OfflineModelType, PageKeys, ParsedShortcut, SwitchKey, ThemeMode,
    TraditionalShortcut, TrayDisplayMode, TypoCorrectionRules, VadConfig, VadModelType,
    VoiceConfig, VoiceEngineType, VoiceInteractionConfig, VoiceItnConfig, VoiceVisualEffect,
};
pub use constants::{MAX_IMPORT_FILE_BYTES, MAX_USER_DICT_BYTES};
pub use input::*;
pub use state::{InputStateSnapshot, SharedInputFlags};
pub use text::pangu::{apply_pangu_spacing, should_insert_pangu_space, PanguSpacing};
pub use text::{is_cjk_character, to_full_width_char, to_full_width_string};
