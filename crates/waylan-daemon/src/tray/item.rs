use crate::config::{AppConfig, env_var_non_empty};
use log::{info, warn};
use parking_lot::RwLock;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use waylan_sys::LoopWakeup;
use zbus::OwnedObjectPath;
use zbus::interface;
use zbus::object_server::SignalEmitter;

/// 提示文本结构类型别名
type ToolTipData = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

use waylan_core::state::SharedInputFlags;
pub use waylan_sys::assets::{detect_default_assets_path, resolve_icon_path_or_name};

/// 解析得到的系统托盘图标配置与资源路径
pub(crate) struct ResolvedTrayIcons {
    pub(crate) theme_path: String,
    pub(crate) icon_zh: String,
    pub(crate) icon_en: String,
    pub(crate) icon_en_caps: String,
}

/// 系统托盘状态项实现 (StatusNotifierItem)
#[derive(Clone)]
pub struct StatusNotifierItem {
    /// 共享输入状态原子容器
    pub(crate) shared_flags: SharedInputFlags,
    /// 图标与状态刷新通知发送端
    pub(crate) icon_update_tx: SyncSender<()>,
    /// 图标主题目录与各状态图标名称（支持配置热重载实时更新）
    pub(crate) icons: Arc<RwLock<ResolvedTrayIcons>>,
    /// 事件循环唤醒通道
    pub(crate) loop_wakeup: Option<LoopWakeup>,
}

/// 托盘状态图标的名称解析输入参数包
///
/// 封装单个状态图标（中文、英文、大写英文）在环境变量、应用配置与内置默认值之间的取值优先级。
struct TrayIconSpec {
    /// 环境变量中显式指定的图标名称（优先级最高）
    env_name: Option<String>,
    /// 应用配置文件中指定的图标名称（次优先级）
    config_name: Option<String>,
    /// 存在自定义图标主题时使用的默认图标名称
    themed_default: &'static str,
    /// 不存在自定义图标主题时使用的系统内置图标名称
    plain_fallback: &'static str,
}

/// 解析托盘图标主题目录路径，并返回当前是否存在自定义图标主题
///
/// 主题来源优先级：环境变量显式指定 > 环境变量图标路径的父目录 > 应用配置 > 随包发布的默认资源目录。
fn resolve_icon_theme_path(config: &AppConfig) -> (String, bool) {
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

    if let Some(p) = explicit_theme {
        (p, true)
    } else if let Some(p) = detect_default_assets_path() {
        (p, true)
    } else {
        (String::new(), false)
    }
}

/// 依据单个状态图标的规格解析出最终可用的图标名称或绝对路径
fn resolve_state_icon(theme_path: &str, has_custom_theme: bool, spec: TrayIconSpec) -> String {
    spec.env_name
        .or(spec.config_name)
        .map(|name| resolve_icon_path_or_name(theme_path, &name))
        .unwrap_or_else(|| {
            if has_custom_theme {
                resolve_icon_path_or_name(theme_path, spec.themed_default)
            } else {
                spec.plain_fallback.to_string()
            }
        })
}

/// 解析系统环境变量与应用配置中的托盘图标主题路径及各状态图标名称
pub(crate) fn resolve_tray_icons(config: &AppConfig) -> ResolvedTrayIcons {
    let (icon_theme_path, has_custom_theme) = resolve_icon_theme_path(config);

    let env_zh = env_var_non_empty("WAYLAN_ICON_ZH");
    let env_en = env_var_non_empty("WAYLAN_ICON_EN");
    let env_en_caps = env_var_non_empty("WAYLAN_ICON_EN_CAPS");

    let icon_zh = resolve_state_icon(
        &icon_theme_path,
        has_custom_theme,
        TrayIconSpec {
            env_name: env_zh,
            config_name: config.desktop.tray_icon_zh.clone(),
            themed_default: "tray-zh",
            plain_fallback: "input-keyboard",
        },
    );

    let icon_en = resolve_state_icon(
        &icon_theme_path,
        has_custom_theme,
        TrayIconSpec {
            env_name: env_en,
            config_name: config.desktop.tray_icon_en.clone(),
            themed_default: "tray-en",
            plain_fallback: "input-latin",
        },
    );

    let icon_en_caps = resolve_state_icon(
        &icon_theme_path,
        has_custom_theme,
        TrayIconSpec {
            env_name: env_en_caps,
            config_name: None,
            themed_default: "tray-en-caps",
            plain_fallback: "input-latin",
        },
    );

    ResolvedTrayIcons {
        theme_path: icon_theme_path,
        icon_zh,
        icon_en,
        icon_en_caps,
    }
}

impl StatusNotifierItem {
    /// 构造 `StatusNotifierItem` 托盘服务对象
    #[cfg(test)]
    pub(crate) fn new(
        shared_flags: SharedInputFlags,
        icon_update_tx: SyncSender<()>,
        config: &AppConfig,
        loop_wakeup: Option<LoopWakeup>,
    ) -> Self {
        Self::with_shared_icons(
            shared_flags,
            icon_update_tx,
            Arc::new(RwLock::new(resolve_tray_icons(config))),
            loop_wakeup,
        )
    }

    /// 使用共享的图标解析状态构造托盘服务对象（支持配置热重载即时替换图标）
    pub(crate) fn with_shared_icons(
        shared_flags: SharedInputFlags,
        icon_update_tx: SyncSender<()>,
        icons: Arc<RwLock<ResolvedTrayIcons>>,
        loop_wakeup: Option<LoopWakeup>,
    ) -> Self {
        Self {
            shared_flags,
            icon_update_tx,
            icons,
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
        self.icons.read().theme_path.clone()
    }

    /// 图标名称（根据中英文与 CapsLock 大写状态动态切换，支持自定义图标）
    #[zbus(property)]
    pub(crate) fn icon_name(&self) -> String {
        let snap = self.shared_flags.snapshot();
        let icons = self.icons.read();
        if snap.is_caps_locked {
            icons.icon_en_caps.clone()
        } else if snap.is_chinese {
            icons.icon_zh.clone()
        } else {
            icons.icon_en.clone()
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
    async fn new_icon(signal_ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// 悬停提示更新通知信号
    #[zbus(signal)]
    async fn new_tool_tip(signal_ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
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
