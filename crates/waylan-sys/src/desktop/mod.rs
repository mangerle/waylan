//! 桌面环境探测、通用 GNOME 原语与串行化后台任务执行器
//!
//! # 设计原理
//! - **实现初衷**：提供无产品策略硬编码的底层桌面探测、扩展同步与串行任务队列。
//! - **核心优势**：所有异步桌面设置写操作通过单线程有界队列串行消费，彻底消除高频切换
//!   设置时裸起多个孤儿线程导致的 `gsettings` 并发写竞态。
//! - **代价与局限**：桌面任务队列为常驻单工作线程，队列容量上限为 16。

pub mod extension;
pub mod gsettings;

use std::env;
use std::process::Command;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::OnceLock;
use std::thread;

use log::warn;

use crate::process::{run_command_with_timeout, DEFAULT_COMMAND_TIMEOUT};

pub use extension::{ensure_gnome_extension_files, set_gnome_extension_enabled};
pub use gsettings::{
    ensure_input_source_prioritized, prioritize_source_entry, set_exclusive_input_source,
};

/// 微澜 GNOME Shell 集成扩展 UUID
pub const WAYLAN_GNOME_EXTENSION_UUID: &str = "waylan-hide-input-source@waylan.org";

/// 微澜在 GNOME `org.gnome.desktop.input-sources` 中的输入源项标识
pub const WAYLAN_IBUS_SOURCE_ENTRY: &str = "('ibus', 'waylan')";

/// 微澜在 IBus 守护进程中的引擎名称
pub const WAYLAN_IBUS_ENGINE_NAME: &str = "waylan";

/// 微澜 GNOME Shell 扩展启用意图
///
/// # 设计原理
/// - **实现初衷**：配套 GNOME 扩展同时承担“隐藏顶栏原生输入源指示器”与“接管渲染微澜自定义候选框”两项职责，
///   任一职责启用即须加载扩展，两项设置互不蕴含、互不牵连。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GnomeExtensionIntent {
    /// 是否需要隐藏顶栏原生输入源指示器（对应独占输入源模式）
    pub hide_indicator: bool,
    /// 是否需要由扩展渲染微澜自定义候选框（对应候选框模式为微澜专属）
    pub custom_candidate_view: bool,
}

impl GnomeExtensionIntent {
    /// 扩展是否应当处于加载状态：两职任一需要即须加载
    #[must_use]
    pub fn should_enable(self) -> bool {
        self.hide_indicator || self.custom_candidate_view
    }
}

/// 桌面后台串行任务类型
type DesktopTask = Box<dyn FnOnce() + Send + 'static>;

/// 读取非空环境变量
pub(crate) fn env_var_non_empty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// 检查当前是否处于 GNOME 桌面环境
pub fn is_gnome_desktop() -> bool {
    env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false)
}

/// 通过 `ibus engine <name>` 激活指定的 IBus 引擎（带超时保护）
pub fn activate_ibus_engine(engine_name: &str) {
    let mut cmd = Command::new("ibus");
    cmd.args(["engine", engine_name]);
    let _ = run_command_with_timeout(cmd, DEFAULT_COMMAND_TIMEOUT);
}

/// 获取全局串行桌面任务通道（有界容量 16，单后台工作线程顺序消费）
fn desktop_task_sender() -> Option<&'static SyncSender<DesktopTask>> {
    static SENDER: OnceLock<Option<SyncSender<DesktopTask>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let (tx, rx) = sync_channel::<DesktopTask>(16);
            let spawn_res = thread::Builder::new()
                .name("waylan-desktop-worker".to_string())
                .spawn(move || {
                    for task in rx {
                        task();
                    }
                });
            match spawn_res {
                Ok(_) => Some(tx),
                Err(e) => {
                    warn!("[桌面集成 警告] 启动桌面串行工作线程失败: {}", e);
                    None
                }
            }
        })
        .as_ref()
}

/// 将桌面集成写操作投递到串行后台工作线程执行，防止并发调用引发 `gsettings` 竞态
pub fn enqueue_desktop_task<F>(task: F)
where
    F: FnOnce() + Send + 'static,
{
    let Some(tx) = desktop_task_sender() else {
        return;
    };
    if let Err(e) = tx.try_send(Box::new(task)) {
        warn!(
            "[桌面集成 警告] 桌面异步任务队列已满或关闭，跳过本次调度: {}",
            e
        );
    }
}
