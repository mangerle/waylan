//! 桌面环境外部子进程安全执行与生命周期守护模块
//!
//! # 设计原理
//! - **实现初衷**：统一复用 `waylan-sys::process` 提供的带超时保护与僵尸进程回收的底层原语。
//! - **核心优势**：消除与各协议层和守护进程内部的重复代码，单一真实源保障进程安全。
//! - **代价与局限**：命令执行超时时子进程会被强制终止。

pub use waylan_sys::process::{
    run_command_with_timeout, safe_gsettings_get, safe_gsettings_set,
    DEFAULT_COMMAND_TIMEOUT as DEFAULT_DESKTOP_COMMAND_TIMEOUT,
};
