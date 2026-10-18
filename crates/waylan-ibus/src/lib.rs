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
pub mod error;
pub mod keys;
pub mod monitor;
pub mod screensaver;
pub mod session;
pub mod types;

pub use error::IbusError;

use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use session::run_ibus_session;
use waylan_core::LoopWakeup;

use desktop_integration::{format_ibus_component_xml, format_ibus_engines_xml};

/// IBus 菜单项外部动作处理器（如打开偏好设置界面、查看系统日志）
pub trait IbusMenuActionHandler: Send + Sync {
    /// 打开偏好设置图形界面
    fn open_settings(&self, settings_running: &Arc<AtomicBool>);
    /// 打开系统运行日志查看器
    fn view_log(&self);
}

/// IBus 词库与运行配置热重载驱动接口
pub trait IbusConfigReloader: Send + Sync {
    /// 当检测到配置文件修改或收到重载信号时在互斥锁保护下调用以更新内存数据。
    /// 返回值：若由于配置变更（如托盘与原生指示器切换）需要触发平滑自重启则返回 true。
    fn reload(&self, core: &mut keys::IbusSessionCore) -> bool;

    /// 在释放核心互斥锁守卫后调用的后置操作（可安全执行耗时外部子进程调用或 I/O，绝不阻塞输入法按键）
    fn post_reload(&self) {}
}

/// IBus 会话停机与事件循环唤醒协调器
#[derive(Clone)]
pub struct IbusShutdownCoordinator {
    exit_requested: Arc<AtomicBool>,
    loop_wakeup: Option<LoopWakeup>,
    restart_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl IbusShutdownCoordinator {
    /// 创建新的停机与唤醒协调器实例
    pub fn new(
        exit_requested: Arc<AtomicBool>,
        loop_wakeup: Option<LoopWakeup>,
        restart_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self {
            exit_requested,
            loop_wakeup,
            restart_hook,
        }
    }

    /// 查询当前是否处于退出或停机流程
    #[inline]
    pub fn is_shutting_down(&self) -> bool {
        self.exit_requested.load(Ordering::Acquire)
    }

    /// 请求优雅退出
    pub fn request_shutdown(&self) {
        self.exit_requested.store(true, Ordering::Release);
        self.wake();
    }

    /// 触发平滑自重启（若宿主环境注册了重启钩子）
    pub fn trigger_graceful_restart(&self) {
        if let Some(ref hook) = self.restart_hook {
            hook();
        } else {
            self.request_shutdown();
        }
    }

    /// 向事件循环发送纳秒级唤醒脉冲
    pub fn wake(&self) {
        if let Some(ref wakeup) = self.loop_wakeup {
            wakeup.wake();
        }
    }

    /// 获取底层唤醒器引用
    pub fn loop_wakeup(&self) -> Option<&LoopWakeup> {
        self.loop_wakeup.as_ref()
    }

    /// 获取退出状态标志的共享原子引用
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exit_requested)
    }
}

/// IBus 会话启动配置参数包
pub struct IbusSessionOptions {
    /// 预先配置并装配好词库与算法状态的 IBus 会话核心状态机
    pub session_core: keys::IbusSessionCore,
    /// 是否由 `ibus-daemon` 通过 `--ibus` 参数作为子进程拉起
    pub is_ibus_child_mode: bool,
    /// 被监控热重载的配置文件与词库路径列表
    pub watched_paths: Vec<PathBuf>,
    /// 初始文件状态签名
    pub initial_mtime: Option<waylan_core::FileSignature>,
    /// 与系统托盘共享的重载请求标志
    pub reload_requested: Option<Arc<AtomicBool>>,
    /// 进程停机协调器
    pub coordinator: IbusShutdownCoordinator,
    /// 是否静默原生 IBus 属性上报（在独立托盘开启时置为 true 以实现原生顶栏白字指示器自动隐藏）
    pub suppress_properties: bool,
    /// 是否将微澜设为 GNOME 独占单一输入源
    pub exclusive_input_source: bool,
    /// 外部菜单动作处理器（打开配置/日志）
    pub menu_handler: Option<Arc<dyn IbusMenuActionHandler>>,
    /// 外部热重载与自重启决策驱动器
    pub config_reloader: Option<Arc<dyn IbusConfigReloader>>,
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
