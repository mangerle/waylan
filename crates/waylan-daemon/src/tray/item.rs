use crate::config::{env_var_non_empty, AppConfig};
use log::{info, warn};
use std::path::Path;
use std::sync::mpsc::SyncSender;
use waylan_wayland::LoopWakeup;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::OwnedObjectPath;

/// 提示文本结构类型别名
type ToolTipData = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

use waylan_core::state::SharedInputFlags;
pub use waylan_sys::assets::{detect_default_assets_path, resolve_icon_path_or_name};

/// 系统托盘状态项实现 (StatusNotifierItem)
#[derive(Clone)]
pub struct StatusNotifierItem {
    /// 共享输入状态原子容器
    pub(crate) shared_flags: SharedInputFlags,
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
        .or_else(|| config.desktop.tray_icon_theme_path.clone());

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
        .or_else(|| config.desktop.tray_icon_zh.clone())
        .map(|name| resolve_icon_path_or_name(&icon_theme_path, &name))
        .unwrap_or_else(|| {
            if has_custom_theme {
                resolve_icon_path_or_name(&icon_theme_path, "tray-zh")
            } else {
                "input-keyboard".to_string()
            }
        });

    let icon_en = env_en
        .or_else(|| config.desktop.tray_icon_en.clone())
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
        shared_flags: SharedInputFlags,
        icon_update_tx: SyncSender<()>,
        config: &AppConfig,
        loop_wakeup: Option<LoopWakeup>,
    ) -> Self {
        let icons = resolve_tray_icons(config);
        Self {
            shared_flags,
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
        let snap = self.shared_flags.snapshot();
        if snap.is_caps_locked {
            self.icon_en_caps.clone()
        } else if snap.is_chinese {
            self.icon_zh.clone()
        } else {
            self.icon_en.clone()
        }
    }

    /// 提示文本结构
    #[zbus(property)]
    fn tool_tip(&self) -> ToolTipData {
        let snap = self.shared_flags.snapshot();
        let mode_str = if snap.is_caps_locked {
            "英文大写 (CapsLock)"
        } else if snap.is_chinese {
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
        let current = self.shared_flags.is_chinese();
        let next = !current;
        self.shared_flags.set_chinese(next);
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
        let flags = SharedInputFlags::new(true, false, false, false);
        let (tx, _rx) = mpsc::sync_channel(1);
        let cfg = AppConfig {
            desktop: waylan_core::config::DesktopConfig {
                tray_icon_theme_path: Some("/opt/waylan/icons".to_string()),
                tray_icon_zh: Some("waylan-zh-custom".to_string()),
                tray_icon_en: Some("waylan-en-custom".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let item = StatusNotifierItem::new(flags.clone(), tx, &cfg, None);
        assert_eq!(item.icon_theme_path(), "/opt/waylan/icons");
        assert_eq!(item.icon_name(), "waylan-zh-custom");

        flags.set_chinese(false);
        assert_eq!(item.icon_name(), "waylan-en-custom");

        flags.set_caps_locked(true);
        assert_eq!(item.icon_name(), "tray-en-caps");
    }

    #[test]
    fn test_auto_detected_assets_path() {
        let flags = SharedInputFlags::new(true, false, false, false);
        let (tx, _rx) = mpsc::sync_channel(1);
        let cfg = AppConfig::default();

        let item = StatusNotifierItem::new(flags.clone(), tx, &cfg, None);

        assert!(item.icon_theme_path().ends_with("assets"));
        assert!(
            item.icon_name().ends_with("tray-zh.svg") || item.icon_name() == "tray-zh",
            "应优先返回规范绝对路径以确保各桌面托盘直接加载"
        );

        flags.set_chinese(false);
        assert!(item.icon_name().ends_with("tray-en.svg") || item.icon_name() == "tray-en");

        flags.set_caps_locked(true);
        assert!(
            item.icon_name().ends_with("tray-en-caps.svg") || item.icon_name() == "tray-en-caps"
        );
    }
}
