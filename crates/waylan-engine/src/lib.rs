//! 微澜 (WayLan) 输入法统一交互运行时与多协议会话引擎
//!
//! # 设计原理
//! - **实现初衷**：将输入法会话状态机、按键决策流水线、统一语音交互与候选框构建逻辑
//!   从具体的算法库 (`waylan-algo`) 与具体的协议端 (`waylan-wayland`、`waylan-ibus`) 中彻底剥离。
//! - **核心优势**：依赖倒置原则（DIP）；算法层仅保留纯粹数学与拼音计算，协议端退化为纯净的前端显示管道。
//! - **代价与局限**：新增单独的 crate 作为中枢交互调度层。

pub mod factory;
pub mod frontend;
pub mod input;
pub mod runtime;
pub mod session;

pub use input::{PunctuationOptions, PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};

pub use factory::{
    apply_default_engine_reload, apply_default_runtime_reload, create_engine_with_table,
    create_standard_engine, ensure_user_dictionaries_linked, load_auxiliary_dict,
    load_auxiliary_managers, load_lexicon_table, load_opencc_dict, reload_engine_resources,
};
pub use frontend::InputMethodFrontend;
pub use runtime::{
    build_common_candidate_box_state, CandidateBoxBuildOptions, ImeRuntime, ProcessKeyEventParams,
};
pub use session::{
    extract_surrounding_prefix, handle_composition_key_with_params, CompositionKeyParams,
    ImeCoreSession, ImeOutputAction, KeyProcessContext, ShortcutToggleAction,
};
pub use waylan_algo as algo;
pub use waylan_algo::{AlgoConfig, AlgoEngine};
pub use waylan_core::input::{VoiceKeyAction, VoiceProvider};
#[cfg(feature = "voice")]
pub use waylan_voice as voice;
#[cfg(feature = "voice")]
pub use waylan_voice::VoiceInteractionManager;
