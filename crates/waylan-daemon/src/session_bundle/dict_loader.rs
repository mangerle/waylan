//! 输入法多级词库加载、符号链接与时间戳感知缓存
//!
//! # 设计原理
//! - 统一转接并复用 [`waylan_engine::factory`] 下沉的权威词库加载器与符号链接构建逻辑。

pub use waylan_engine::load_lexicon_table;

#[cfg(test)]
pub use waylan_engine::{load_auxiliary_dict, load_opencc_dict};
