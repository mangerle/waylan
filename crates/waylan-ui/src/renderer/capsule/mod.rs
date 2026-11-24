//! 胶囊候选栏与语音声波胶囊渲染模块
//!
//! # 设计原理
//! - **实现初衷**：将常规单行打字候选条与语音听写声波胶囊进行关注点分离拆分。
//! - **核心优势**：
//!   1. 降低单文件代码行数与圈复杂度；
//!   2. 常规拼音选词与流式语音转录渲染逻辑彻底解耦，互不侵入。

mod capsule_bar;
mod voice_capsule;

pub use capsule_bar::{CapsuleBarParams, render_capsule_bar};
pub(crate) use voice_capsule::has_active_time_animation;
pub use voice_capsule::render_voice_capsule_bar;
