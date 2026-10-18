//! 输入法预处理器兼容导出（防误触状态机与智能标点处理器已下沉至 `waylan_core::input`）
//!
//! # 架构下沉说明
//! `ShiftToggleStateMachine` 与 `PunctuationProcessor` 是纯前端输入管道与修饰键流转状态机，
//! 不依赖任何底层拼音、词典或动态规划算法。为解除核心算法库对前端流控的越权侵占，
//! 相关实现已收敛至 `waylan_core::input`，本模块保留完全兼容的符号重导出。

pub use waylan_core::input::{PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};
