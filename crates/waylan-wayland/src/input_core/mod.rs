//! 核心输入决策与按键处理模块
//!
//! 包含翻页配置、Linux 扫描码转换、拼音组词按键状态机与物理修饰键位图跟踪。

pub mod composition;
pub mod conversion;
pub mod modifiers;
pub mod page_keys;

pub use composition::*;
pub use conversion::*;
pub use modifiers::*;
pub use page_keys::*;

pub use crate::input_constants::*;
pub use crate::keymap::KeymapLayout;

#[cfg(test)]
#[path = "../input_core_tests.rs"]
mod tests;
