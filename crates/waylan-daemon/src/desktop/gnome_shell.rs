//! GNOME 桌面环境原生输入源与隐藏指示器扩展集成管理
//!
//! # 设计原理
//! - **实现初衷**：统一委托底层系统抽象库 `waylan-sys::desktop`，实现守护进程与设置界面的零差异共享。
//! - **核心优势**：消除重复代码，保障单一真相源，避免输入源覆盖与子进程超时隐患。
//!   配置到扩展意图的映射集中在本模块，`waylan-sys` 保持不感知应用配置层。

use waylan_core::config::AppConfig;
pub use waylan_sys::desktop::GnomeExtensionIntent;
use waylan_sys::desktop::{
    WAYLAN_GNOME_EXTENSION_UUID, WAYLAN_IBUS_SOURCE_ENTRY, enqueue_desktop_task,
    ensure_gnome_extension_files, ensure_input_source_prioritized, is_gnome_desktop,
    set_gnome_extension_enabled,
};

/// 微澜扩展资源子目录名
const EXTENSION_ASSETS_SUBDIR: &str = "gnome-extension";
/// 默认兜底 XKB 输入源条目
const FALLBACK_XKB_ENTRY: &str = "('xkb', 'us')";

/// 检查并在 GNOME `org.gnome.desktop.input-sources` 中将微澜注册并置顶为首选输入源
pub fn ensure_gnome_input_source_registered(exclusive: bool) {
    ensure_input_source_prioritized(WAYLAN_IBUS_SOURCE_ENTRY, FALLBACK_XKB_ENTRY, exclusive);
}

/// 同步微澜 GNOME Shell 扩展的加载状态
pub fn sync_gnome_hide_indicator_extension(intent: GnomeExtensionIntent) {
    if !is_gnome_desktop() {
        return;
    }
    ensure_gnome_extension_files(WAYLAN_GNOME_EXTENSION_UUID, EXTENSION_ASSETS_SUBDIR);
    set_gnome_extension_enabled(WAYLAN_GNOME_EXTENSION_UUID, intent.should_enable());
}

/// 后台串行派发扩展加载状态对齐（不触碰输入源与当前引擎选择）
pub fn sync_gnome_extension_state_async(intent: GnomeExtensionIntent) {
    if !is_gnome_desktop() {
        return;
    }
    enqueue_desktop_task(move || sync_gnome_hide_indicator_extension(intent));
}

/// 由应用配置解析 GNOME 扩展启用意图
///
/// 扩展的"隐藏原生指示器"与"接管候选框渲染"两职分别对应 `desktop.exclusive_input_source`
/// 与 `ui.candidate_window_mode`，二者互不蕴含，故必须同时读取。独占意图走
/// `effective_exclusive_source`，以便运维环境变量强制位与托盘形态保持一致。
#[must_use]
pub fn gnome_extension_intent(cfg: &AppConfig) -> GnomeExtensionIntent {
    GnomeExtensionIntent {
        hide_indicator: crate::system::effective_exclusive_source(cfg),
        custom_candidate_view: cfg.uses_custom_candidate_view(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::config::CandidateWindowMode;

    #[test]
    fn test_intent_requires_extension_for_custom_candidate_view() {
        // 未开独占输入源但选择微澜候选框：扩展仍须加载，否则外观设置静默失效
        let cfg = AppConfig::default();
        let intent = gnome_extension_intent(&cfg);
        assert!(intent.custom_candidate_view);
        assert!(
            intent.should_enable(),
            "默认配置为微澜自定义候选框，扩展必须处于加载状态"
        );
    }

    #[test]
    fn test_intent_disables_extension_when_both_duties_off() {
        let mut cfg = AppConfig::default();
        cfg.desktop.exclusive_input_source = false;
        cfg.ui.candidate_window_mode = CandidateWindowMode::Native;
        assert!(
            !gnome_extension_intent(&cfg).should_enable(),
            "两职责皆关时不应加载扩展"
        );
    }

    #[test]
    fn test_intent_keeps_indicator_hidden_only_when_exclusive() {
        let mut cfg = AppConfig::default();
        cfg.desktop.exclusive_input_source = true;
        let intent = gnome_extension_intent(&cfg);
        assert!(intent.hide_indicator, "独占模式才允许隐藏顶栏指示器");
    }
}
