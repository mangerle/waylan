//! 增量敲击编辑与组合状态机类型别名与导出
//!
//! # 设计原理
//! - **实现初衷**：为上层提供向后兼容的 [`Composition`] 别名，底层统一收敛至 [`crate::engine::CompositionBuffer`]。
//! - **核心优势**：消除平行多层代理包裹，保持外部 API 语义稳定。

pub use crate::engine::composition_buffer::CompositionBuffer as Composition;
