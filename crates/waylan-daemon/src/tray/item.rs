use crate::config::{env_var_non_empty, AppConfig};
use log::{info, warn};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use waylan_wayland::LoopWakeup;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::OwnedObjectPath;

/// 提示文本结构类型别名
type ToolTipData = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

pub use waylan_core::assets::{detect_default_assets_path, resolve_icon_path_or_name};

/// 系统托盘状态项实现 (StatusNotifierItem)
#[derive(Clone)]
pub struct StatusNotifierItem {
    /// 当前是否处于中文模式
    pub(crate) is_chinese: Arc<AtomicBool>,
    /// 当前是否处于 CapsLock 大写锁定状态
    pub(crate) is_caps_locked: Arc<AtomicBool>,
    /// 图标与状态刷新通知发送端
    pub(crate) icon_update_tx: SyncSender<()>,
    /// 自定义图标主题目录
    pub(crate) icon_theme_path: String,
    /// 自定义中文图标名称
    pub(crate) icon_zh: String,
    /// 自定义英文图标名称
    pub(crate) icon_en: String,
    /// 自定义英文大写图标名称
    pub(crate) icon_en_caps: String,
    /// 事件循环唤醒通道
    pub(crate) loop_wakeup: Option<LoopWakeup>,
}

/// 解析得到的系统托盘图标配置与资源路径
struct ResolvedTrayIcons {
    theme_path: String,
    icon_zh: String,
    icon_en: String,
    icon_en_caps: String,
}

/// 解析系统环境变量与应用配置中的托盘图标主题路径及各状态图标名称
fn resolve_tray_icons(config: &AppConfig) -> ResolvedTrayIcons {
    let env_theme = env_var_non_empty("WAYLAN_ICON_THEME_PATH");
    let env_path = env_var_non_empty("WAYLAN_ICON_PATH");
    let path_parent = env_path.as_deref().and_then(|p| {
        Path::new(p)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().into_owned())
    });

    let explicit_theme = env_theme
        .or(path_parent)
        .or_else(|| config.tray_icon_theme_path.clone());

    let (icon_theme_path, has_custom_theme) = if let Some(p) = explicit_theme {
        (p, true)
    } else if let Some(p) = detect_default_assets_path() {
        (p, true)
    } else {
        (String::new(), false)
    };

    let env_zh = env_var_non_empty("WAYLAN_ICON_ZH");
    let env_en = env_var_non_empty("WAYLAN_ICON_EN");
    let env_en_caps = env_var_non_empty("WAYLAN_ICON_EN_CAPS");

    let icon_zh = env_zh
        .or_else(|| config.tray_icon_zh.clone())
        .map(|name| resolve_icon_path_or_name(&icon_theme_path, &name))
        .unwrap_or_else(|| {
            if has_custom_theme {
                resolve_icon_path_or_name(&icon_theme_path, "tray-zh")
            } else {
                "input-keyboard".to_string()
            }
        });

    let icon_en = env_en
        .or_else(|| config.tray_icon_en.clone())
        .map(|name| resolve_icon_path_or_name(&icon_theme_path, &name))
        .unwrap_or_else(|| {
            if has_custom_theme {
                resolve_icon_path_or_name(&icon_theme_path, "tray-en")
            } else {
                "input-latin".to_string()
            }
        });

    let icon_en_caps = env_en_caps
        .map(|name| resolve_icon_path_or_name(&icon_theme_path, &name))
        .unwrap_or_else(|| {
            if has_custom_theme {
                resolve_icon_path_or_name(&icon_theme_path, "tray-en-caps")
            } else {
                "input-latin".to_string()
            }
        });

    ResolvedTrayIcons {
        theme_path: icon_theme_path,
        icon_zh,
        icon_en,
        icon_en_caps,
    }
}

impl StatusNotifierItem {
    pub(crate) fn new(
        is_chinese: Arc<AtomicBool>,
        is_caps_locked: Arc<AtomicBool>,
        icon_update_tx: SyncSender<()>,
        config: &AppConfig,
        loop_wakeup: Option<LoopWakeup>,
    ) -> Self {
        let icons = resolve_tray_icons(config);
        Self {
            is_chinese,
            is_caps_locked,
            icon_update_tx,
            icon_theme_path: icons.theme_path,
            icon_zh: icons.icon_zh,
            icon_en: icons.icon_en,
            icon_en_caps: icons.icon_en_caps,
            loop_wakeup,
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

    /// 图标主题额外搜索目录 (IconThemePath)
    #[zbus(property)]
    pub(crate) fn icon_theme_path(&self) -> String {
        self.icon_theme_path.clone()
    }

    /// 图标名称（根据中英文与 CapsLock 大写状态动态切换，支持自定义图标）
    #[zbus(property)]
    pub(crate) fn icon_name(&self) -> String {
        if self.is_caps_locked.load(Ordering::Acquire) {
            self.icon_en_caps.clone()
        } else if self.is_chinese.load(Ordering::Acquire) {
            self.icon_zh.clone()
        } else {
            self.icon_en.clone()
        }
    }

    /// 提示文本结构
    #[zbus(property)]
    fn tool_tip(&self) -> ToolTipData {
        let mode_str = if self.is_caps_locked.load(Ordering::Acquire) {
            "英文大写 (CapsLock)"
        } else if self.is_chinese.load(Ordering::Acquire) {
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
        if let Some(ref wakeup) = self.loop_wakeup {
            wakeup.wake();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_custom_tray_icon_and_theme_path() {
        let is_chinese = Arc::new(AtomicBool::new(true));
        let is_caps_locked = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::sync_channel(1);
        let cfg = AppConfig {
            tray_icon_theme_path: Some("/opt/waylan/icons".to_string()),
            tray_icon_zh: Some("waylan-zh-custom".to_string()),
            tray_icon_en: Some("waylan-en-custom".to_string()),
            ..Default::default()
        };

        let item = StatusNotifierItem::new(
            Arc::clone(&is_chinese),
            Arc::clone(&is_caps_locked),
            tx,
            &cfg,
            None,
        );
        assert_eq!(item.icon_theme_path(), "/opt/waylan/icons");
        assert_eq!(item.icon_name(), "waylan-zh-custom");

        is_chinese.store(false, Ordering::Release);
        assert_eq!(item.icon_name(), "waylan-en-custom");

        is_caps_locked.store(true, Ordering::Release);
        assert_eq!(item.icon_name(), "tray-en-caps");
    }

    #[test]
    fn test_auto_detected_assets_path() {
        let is_chinese = Arc::new(AtomicBool::new(true));
        let is_caps_locked = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::sync_channel(1);
        let cfg = AppConfig::default();

        let item = StatusNotifierItem::new(
            Arc::clone(&is_chinese),
            Arc::clone(&is_caps_locked),
            tx,
            &cfg,
            None,
        );

        assert!(item.icon_theme_path().ends_with("assets"));
        assert!(
            item.icon_name().ends_with("tray-zh.svg") || item.icon_name() == "tray-zh",
            "应优先返回规范绝对路径以确保各桌面托盘直接加载"
        );

        is_chinese.store(false, Ordering::Release);
        assert!(item.icon_name().ends_with("tray-en.svg") || item.icon_name() == "tray-en");

        is_caps_locked.store(true, Ordering::Release);
        assert!(
            item.icon_name().ends_with("tray-en-caps.svg") || item.icon_name() == "tray-en-caps"
        );
    }
}
