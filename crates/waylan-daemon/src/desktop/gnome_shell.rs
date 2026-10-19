//! GNOME 桌面环境原生输入源与隐藏指示器扩展集成管理
//!
//! # 设计原理
//! - **实现初衷**：统一委托底层系统抽象库 `waylan-sys::desktop`，实现守护进程与设置界面的零差异共享。
//! - **核心优势**：消除重复代码，保障单一真相源，避免输入源覆盖与子进程超时隐患。

pub use waylan_sys::desktop::{
    ensure_gnome_input_source_registered, sync_gnome_hide_indicator_extension,
};
