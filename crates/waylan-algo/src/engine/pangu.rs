//! 盘古之白（中西文智能空格）与中文量词过滤模块
//!
//! # 设计原理
//! - **实现初衷**：为算法引擎候选词合成与上屏文本插入智能空格。
//! - **核心优势**：直接复用 `waylan_core::text::pangu` 权威实现，彻底消除跨 Crate 规则失步。

pub use waylan_core::PanguSpacing;
