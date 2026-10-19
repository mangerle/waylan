//! 微澜输入法按键交互流控与状态机模块
//!
//! # 架构设计
//! 包含中英文修饰键快速切换状态机 (`ShiftToggleStateMachine`) 与智能标点符号处理器 (`PunctuationProcessor`)。
//! 作为会话调度层的一级公民，负责在底层拼音引擎与前端协议之间执行按键拦截与符号映射。

pub mod punctuation;
pub mod shift_toggle;

pub use punctuation::{PunctuationOptions, PunctuationProcessor};
pub use shift_toggle::{ShiftToggleStateMachine, ToggleKeyMode};
