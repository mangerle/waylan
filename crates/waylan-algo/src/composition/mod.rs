//! 增量敲击编辑、组合状态机与按键交互决策
//!
//! # 设计原理
//! - **实现初衷**：统一拼音输入的文本光标状态机（[`Composition`]）与按键调度动作决策（[`KeyProcessAction`]）。
//! - **核心优势**：为 Wayland 与 IBus 平台提供协议无关、行为绝对一致的拼音交互能力。

pub mod key_handler;
pub mod state;

pub use key_handler::{
    handle_composition_key, handle_composition_key_with_params, CompositionKeyParams,
    KeyProcessAction,
};
pub use state::Composition;
