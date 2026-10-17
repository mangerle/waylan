//! IBus 键盘事件映射与输入法会话核心按键状态机处理
//!
//! # 设计原理
//! - **实现初衷**：IBus `ProcessKeyEvent(keyval, keycode, state)` 传入的是 X11/GDK `keyval` 符号与修饰位掩码，
//!   而微澜的核心按键决策层 (`waylan_algo::composition`) 基于 Linux evdev 扫描码工作。本模块负责将 IBus 按键事件
//!   无损归一化为标准扫描码，并驱动 `AlgoEngine`、`ShiftToggleStateMachine` 与 `PunctuationProcessor`。
//! - **核心优势**：优先基于与键盘物理布局解耦的 `keyval` 进行映射，自动兼容 QWERTY、Dvorak、Colemak 及 XWayland 客户端，
//!   100% 复用微澜现有的拼音组词、V 模式、简繁转换及智能标点逻辑。
//! - **代价与局限**：成对标点补全的光标回退需通过 `ForwardKeyEvent` 模拟左方向键完成。

pub mod core;
pub mod dispatcher;
pub mod keyval;
pub mod voice;

#[cfg(test)]
mod tests;

pub use core::{IbusKeyOutcome, IbusSessionCore};
pub use keyval::{IBUS_KEYVAL_LEFT, IBUS_KEYVAL_RIGHT, IBUS_RELEASE_MASK};
