use log::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::OwnedObjectPath;

/// 提示文本结构类型别名
type ToolTipData = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

/// 系统托盘状态项实现 (StatusNotifierItem)
pub struct StatusNotifierItem {
    /// 当前是否处于中文模式
    pub(crate) is_chinese: Arc<AtomicBool>,
    /// 图标与状态刷新通知发送端
    pub(crate) icon_update_tx: SyncSender<()>,
}

impl StatusNotifierItem {
    pub(crate) fn new(is_chinese: Arc<AtomicBool>, icon_update_tx: SyncSender<()>) -> Self {
        Self {
            is_chinese,
            icon_update_tx,
        }
    }
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl StatusNotifierItem {
    /// 类别
    #[zbus(property)]
    fn category(&self) -> String {
        "ApplicationStatus".to_string()
    }

    /// 唯一标识
    #[zbus(property)]
    fn id(&self) -> String {
        "waylan".to_string()
    }

    /// 显示标题
    #[zbus(property)]
    fn title(&self) -> String {
        "微澜输入法".to_string()
    }

    /// 状态（Active 表示常驻激活）
    #[zbus(property)]
    fn status(&self) -> String {
        "Active".to_string()
    }

    /// 图标名称（根据中英文状态动态切换）
    #[zbus(property)]
    pub(crate) fn icon_name(&self) -> String {
        if self.is_chinese.load(Ordering::Acquire) {
            "input-keyboard".to_string()
        } else {
            "input-latin".to_string()
        }
    }

    /// 提示文本结构
    #[zbus(property)]
    fn tool_tip(&self) -> ToolTipData {
        let mode_str = if self.is_chinese.load(Ordering::Acquire) {
            "中文拼音"
        } else {
            "英文模式"
        };
        (
            "waylan".to_string(),
            Vec::new(),
            "微澜输入法".to_string(),
            format!("当前工作状态: {}", mode_str),
        )
    }

    /// 上下文菜单 DBus 路径 (指向 /MenuBar)
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/MenuBar").unwrap_or_else(|e| {
            warn!(
                "[系统托盘] 解析 /MenuBar 对象路径失败: {}，使用根路径兜底",
                e
            );
            OwnedObjectPath::default()
        })
    }

    /// 用户左键点击托盘图标：触发中英文快速切换
    pub(crate) fn activate(&self, _x: i32, _y: i32) {
        let current = self.is_chinese.load(Ordering::Acquire);
        let next = !current;
        self.is_chinese.store(next, Ordering::Release);
        let _ = self.icon_update_tx.try_send(());
        info!(
            "[系统托盘] 用户点击托盘图标，切换输入模式为: {}",
            if next { "中文" } else { "英文" }
        );
    }

    /// 用户右键点击托盘图标：触发上下文菜单响应
    fn context_menu(&self, x: i32, y: i32) {
        info!("[系统托盘] 用户右键点击托盘图标，坐标: ({}, {})", x, y);
    }

    /// 图标更新通知信号（用于告知系统任务栏刷新图标呈现）
    #[zbus(signal)]
    async fn new_icon(signal_ctxt: &SignalContext<'_>) -> zbus::Result<()>;

    /// 悬停提示更新通知信号
    #[zbus(signal)]
    async fn new_tool_tip(signal_ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}
