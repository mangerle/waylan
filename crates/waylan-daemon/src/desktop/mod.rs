//! 桌面环境集成、GNOME 输入源注册与子进程超时守护调度
//!
//! # 设计原理
//! - **实现初衷**：将对宿主操作系统桌面环境（GNOME Shell、GSettings、系统输入源置顶）的调度
//!   从具体的输入法引擎协议库（如 `waylan-ibus`）中解耦，统一提升至系统守护进程调度层。
//! - **核心优势**：纯 Wayland 会话与守护进程系统生命周期调度不再反直觉地跨层调用 IBus 内部实现。
//! - **代价与局限**：桌面集成操作依赖外部命令工具（`gsettings`、`gnome-extensions`）。

pub mod executor;
pub mod gnome_shell;

pub use executor::{run_command_with_timeout, DEFAULT_DESKTOP_COMMAND_TIMEOUT};
pub use gnome_shell::{ensure_gnome_input_source_registered, sync_gnome_hide_indicator_extension};
