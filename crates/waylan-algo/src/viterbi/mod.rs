//! 维特比整句动态规划解码与统计语言模型模块
//!
//! 整合 2-gram 转移亲和度计算（`BigramLanguageModel`）与自适应束搜索路径求解（`ViterbiDecoder`）。

mod decoder;
mod language_model;
mod tail_expansion;

pub use decoder::ViterbiDecoder;
pub use language_model::BigramLanguageModel;

#[cfg(test)]
mod tests;
