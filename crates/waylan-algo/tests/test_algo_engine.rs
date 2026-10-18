//! 算法引擎核心端到端功能集成测试套件入口
//!
//! # 设计原理
//! - **实现初衷**：遵循关注点分离原则，将 700+ 行巨型单体测试拆分为 4 个高度内聚的独立领域子模块。
//! - **核心优势**：职责清晰、结构扁平，单个测试模块代码量控制在 100~260 行以内，提升维护性与调试效率。

#[path = "algo_engine/basic.rs"]
mod basic;

#[path = "algo_engine/synthesis.rs"]
mod synthesis;

#[path = "algo_engine/typo_and_context.rs"]
mod typo_and_context;

#[path = "algo_engine/user_dict.rs"]
mod user_dict;
