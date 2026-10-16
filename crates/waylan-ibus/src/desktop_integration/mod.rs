//! GNOME 桌面输入源与用户级 IBus 组件注册缓存自动集成模块
//!
//! # 设计原理
//! - **实现初衷**：在 Ubuntu GNOME (Mutter) 桌面下，顶栏右上角的原生输入法指示器菜单（`InputSourceManager`）
//!   仅展示同时存在于 `org.gnome.desktop.input-sources`（`gsettings`）与 `ibus-daemon` 静态组件注册表
//!   （`GetEnginesByNames`）中的输入源。若仅通过 D-Bus 动态调用 `RegisterComponent`，GNOME 顶栏不会将微澜合并进
//!   右上角的 IBus 状态菜单。
//! - **核心优势**：无需 root 权限写入 `/usr/share/ibus/component`，通过生成 `~/.local/share/ibus/component/waylan.xml`
//!   并调用 `ibus write-cache` 更新用户级 `~/.cache/ibus/bus/registry` 缓存，配合 `gsettings` 自动将 `('ibus', 'waylan')`
//!   注册进 GNOME 输入源列表，实现与系统原生 IBus 指示器的无缝合并。
//! - **代价与局限**：首次写入组件 XML 后，若当前 `ibus-daemon` 尚未加载新缓存，需通知或重启 `ibus-daemon` 以刷新静态表。

pub mod component_cache;
pub mod gnome_shell;

pub use component_cache::{format_ibus_component_xml, format_ibus_engines_xml};
pub use gnome_shell::{ensure_gnome_input_source_registered, sync_gnome_hide_indicator_extension};

/// 确保微澜组件注册就绪并同步置顶至 GNOME 输入源列表
pub fn ensure_ibus_and_gnome_integration(exe_path: &str, exclusive: bool) {
    component_cache::ensure_ibus_component_installed(exe_path);
    gnome_shell::ensure_gnome_input_source_registered(exclusive);
    gnome_shell::sync_gnome_hide_indicator_extension(exclusive);
}

#[cfg(test)]
mod tests {
    use super::gnome_shell::prioritize_waylan_in_gsettings_sources;
    use super::*;

    #[test]
    fn test_prioritize_waylan_in_gsettings_sources() {
        // 微澜在末尾时，应被提升至首位
        assert_eq!(
            prioritize_waylan_in_gsettings_sources(
                "[('xkb', 'cn'), ('ibus', 'libpinyin'), ('ibus', 'waylan')]"
            )
            .as_deref(),
            Some("[('ibus', 'waylan'), ('xkb', 'cn'), ('ibus', 'libpinyin')]")
        );

        // 微澜不在列表中时，应插入至首位
        assert_eq!(
            prioritize_waylan_in_gsettings_sources("[('xkb', 'cn'), ('ibus', 'libpinyin')]")
                .as_deref(),
            Some("[('ibus', 'waylan'), ('xkb', 'cn'), ('ibus', 'libpinyin')]")
        );

        // 空列表情况（自动补充兜底英文 XKB）
        assert_eq!(
            prioritize_waylan_in_gsettings_sources("@a(ss) []").as_deref(),
            Some("[('ibus', 'waylan'), ('xkb', 'us')]")
        );

        // 微澜已在首位且包含 xkb 时，无需变更返回 None
        assert!(
            prioritize_waylan_in_gsettings_sources("[('ibus', 'waylan'), ('xkb', 'cn')]").is_none()
        );

        // 若微澜在首位但缺失 xkb，应自动补充兜底英文 XKB
        assert_eq!(
            prioritize_waylan_in_gsettings_sources("[('ibus', 'waylan')]").as_deref(),
            Some("[('ibus', 'waylan'), ('xkb', 'us')]")
        );
    }

    #[test]
    fn test_format_ibus_component_and_engines_xml() {
        let test_version = "9.9.9-test";
        let comp_xml = format_ibus_component_xml("/usr/bin/waylan", test_version);
        assert!(comp_xml.contains("<name>org.freedesktop.IBus.Waylan</name>"));
        assert!(comp_xml.contains("<exec>/usr/bin/waylan --ibus</exec>"));
        assert!(comp_xml.contains(&format!("<version>{test_version}</version>")));

        let eng_xml = format_ibus_engines_xml("/usr/bin/waylan", test_version);
        assert!(eng_xml.contains("<name>waylan</name>"));
        assert!(eng_xml.contains("<setup>/usr/bin/waylan settings</setup>"));
        assert!(eng_xml.contains(&format!("<version>{test_version}</version>")));
    }
}
