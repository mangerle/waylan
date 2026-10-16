//! 键盘字符布局映射管理（适配 QWERTY、Dvorak、Colemak、AZERTY 等动态 XKB 键盘布局）
//!
//! 核心布局管理由 `waylan-core` 统一维护，此处重新导出以保持向前兼容。

pub use waylan_core::input::keymap::KeymapLayout;
