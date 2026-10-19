//! 候选词、候选框展示状态与输入法动作领域模型
//!
//! # 设计原理
//! - **实现初衷**：为输入法核心算法、UI 渲染引擎、语音转录与 Wayland/IBus 协议前端提供统一的候选词与动作契约。
//! - **核心优势**：严格遵循“非法状态不可表示 (Make Illegal States Unrepresentable)”设计原则，
//!   通过强类型枚举彻底排除未展开却存在选中索引等非法状态组合。
//! - **代价与局限**：作为跨 Crate 流转的核心数据契约，字段改动需评估向下兼容性。

pub mod action;
pub mod entity;
pub mod format;
pub mod state;
pub mod view;

#[cfg(test)]
mod tests;

pub use action::{InputMode, TextAction};
pub use entity::{Candidate, CandidateTier};
pub use format::{format_candidate_display, CIRCLED_NUMBERS};
pub use state::{CandidateBoxState, CandidateExpansion, VoiceBoxState};
pub use view::{CandidateBoxView, CandidateViewMode};
