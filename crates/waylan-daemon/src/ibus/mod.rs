//! 微澜 (WayLan) IBus 输入法引擎会话管理与总线自动发现模块
//!
//! # 设计原理
//! - **实现初衷**：Ubuntu 默认的 GNOME (Mutter) 桌面环境不向外部暴露 `zwp_input_method_v1` 或
//!   `zwp_input_method_manager_v2` 协议，而是与 `ibus-daemon` 深度集成。本模块通过 D-Bus 协议实现完整的
//!   IBus 引擎后端，使微澜在 GNOME 桌面下既支持被 `ibus-daemon` 作为组件拉起（`--ibus`），也支持在直接运行
//!   `waylan` 时自动发现活跃 IBus 总线并动态注册激活。
//! - **核心优势**：无需 root 权限即可通过 `RegisterComponent` + `SetGlobalEngine` 动态接入当前 GNOME 会话，
//!   同时复用 `inotify` 与 `eventfd` (`LoopWakeup`) 实现零忙轮询的配置热重载与系统托盘联动。
//! - **代价与局限**：依赖宿主机运行中的 `ibus-daemon` 会话实例。

pub mod desktop_integration;
pub mod discovery;
pub mod engine;
pub mod keys;
pub mod monitor;
pub mod screensaver;
pub mod session;
pub mod types;

use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::SystemTime;

use waylan_algo::AlgoEngine;

use crate::system::ShutdownCoordinator;

pub use session::run_ibus_session;

use desktop_integration::{format_ibus_component_xml, format_ibus_engines_xml};

/// IBus 会话启动配置参数包
pub struct IbusSessionOptions {
    /// 已加载词库的拼音算法引擎
    pub engine: AlgoEngine,
    /// 是否由 `ibus-daemon` 通过 `--ibus` 参数作为子进程拉起
    pub is_ibus_child_mode: bool,
    /// 被监控热重载的配置文件与词库路径列表
    pub watched_paths: Vec<PathBuf>,
    /// 初始文件修改时间戳
    pub initial_mtime: Option<SystemTime>,
    /// 与系统托盘共享的中英文模式标志
    pub shared_is_chinese: Option<Arc<AtomicBool>>,
    /// 与系统托盘共享的 CapsLock 大写锁定标志
    pub shared_is_caps_locked: Option<Arc<AtomicBool>>,
    /// 唤醒系统托盘刷新图标的通知通道
    pub mode_notify_tx: Option<SyncSender<()>>,
    /// 与系统托盘共享的重载请求标志
    pub reload_requested: Option<Arc<AtomicBool>>,
    /// 与系统托盘共享的简繁模式标志
    pub shared_traditional_chinese: Option<Arc<AtomicBool>>,
    /// 与系统托盘共享的全角/半角标点标志
    pub shared_full_width_punct: Option<Arc<AtomicBool>>,
    /// 与系统托盘共享的全角/半角字符标志
    pub shared_full_width_char: Option<Arc<AtomicBool>>,
    /// 进程停机协调器
    pub coordinator: ShutdownCoordinator,
    /// 是否静默原生 IBus 属性上报（在独立托盘开启时置为 true 以实现原生顶栏白字指示器自动隐藏）
    pub suppress_properties: bool,
    /// 是否将微澜设为 GNOME 独占单一输入源
    pub exclusive_input_source: bool,
}

/// 输出符合 IBus `<engines>` 规范的 XML 引擎列表（响应 `waylan --xml`）
pub fn print_ibus_engines_xml() {
    let exe_path = env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "/usr/bin/waylan".to_string());
    let version = env!("CARGO_PKG_VERSION");
    let xml = format_ibus_engines_xml(&exe_path, version);
    let _ = io::stdout().write_all(xml.as_bytes());
}

/// 输出符合 IBus `<component>` 规范的 XML 组件描述文本（响应 `waylan --ibus-component-xml`）
pub fn print_ibus_component_xml() {
    let version = env!("CARGO_PKG_VERSION");
    let xml = format_ibus_component_xml("/usr/bin/waylan", version);
    let _ = io::stdout().write_all(xml.as_bytes());
}
