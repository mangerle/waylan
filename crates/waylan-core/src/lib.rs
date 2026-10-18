//! 微澜 (WayLan) 核心数据类型、输入法状态与配置系统

pub mod assets;
pub mod candidate;
pub mod candidate_format;
pub mod config;
pub mod constants;
pub mod fs_watcher;
pub mod input;
pub mod loop_wakeup;
pub mod state;
pub mod text;

pub use candidate::{
    Candidate, CandidateBoxState, ExpandedCardState, GridState, InputMode, TextAction,
    VoiceBoxState,
};
pub use candidate_format::{format_candidate_display, CIRCLED_NUMBERS};
pub use config::{
    AppConfig, AudioFrontEndConfig, CandidateIndexStyle, CandidateOrientation, CloudAsrConfig,
    CloudProfile, CloudProviderType, ConfigError, FullWidthCharShortcut, FullWidthPunctShortcut,
    FuzzyPinyinRules, NsEngineType, OfflineAsrConfig, OfflineModelType, PageKeys, ParsedShortcut,
    SwitchKey, ThemeMode, TraditionalShortcut, TrayDisplayMode, TypoCorrectionRules, VadConfig,
    VadModelType, VoiceConfig, VoiceEngineType, VoiceInteractionConfig, VoiceItnConfig,
    VoiceVisualEffect,
};
pub use constants::{MAX_IMPORT_FILE_BYTES, MAX_USER_DICT_BYTES};
pub use fs_watcher::{
    check_reload_requested_or_files_changed, check_watched_paths_changed,
    compute_initial_signature, init_session_inotify_watcher, FileSignature, InotifyWatcher,
};
pub use input::*;
pub use loop_wakeup::LoopWakeup;
pub use state::{InputStateSnapshot, SharedInputFlags};
pub use text::pangu::{apply_pangu_spacing, should_insert_pangu_space, PanguSpacing};
pub use text::{is_cjk_character, to_full_width_char, to_full_width_string};
