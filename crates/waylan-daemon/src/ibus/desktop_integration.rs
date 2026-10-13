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

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use log::{debug, info, warn};

use crate::config::env_var_non_empty;

/// 生成完整的 IBus `<component>` XML 文本
pub fn format_ibus_component_xml(exe_path: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<component>
    <name>org.freedesktop.IBus.Waylan</name>
    <description>WayLan Chinese Input Method Component</description>
    <exec>{exe} --ibus</exec>
    <version>{ver}</version>
    <author>WayLan Contributors</author>
    <license>MIT OR Apache-2.0</license>
    <homepage>https://github.com/waylan</homepage>
    <textdomain>waylan</textdomain>
    <engines>
        <engine>
            <name>waylan</name>
            <longname>微澜拼音 (WayLan)</longname>
            <description>微澜 (WayLan) - 现代轻量纯 Rust 中文拼音输入法</description>
            <language>zh_CN</language>
            <license>MIT OR Apache-2.0</license>
            <author>WayLan Contributors</author>
            <icon>input-keyboard</icon>
            <layout>us</layout>
            <rank>99</rank>
            <symbol>&#x6F9C;</symbol>
            <icon_prop_key>InputMode</icon_prop_key>
            <setup>{exe} settings</setup>
            <version>{ver}</version>
            <textdomain>waylan</textdomain>
        </engine>
    </engines>
</component>
"#,
        exe = exe_path,
        ver = version
    )
}

/// 生成符合 IBus `<engines>` 规范的 XML 描述文本（响应 `waylan --xml`）
pub fn format_ibus_engines_xml(exe_path: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<engines>
    <engine>
        <name>waylan</name>
        <longname>微澜拼音 (WayLan)</longname>
        <description>微澜 (WayLan) - 现代轻量纯 Rust 中文拼音输入法</description>
        <language>zh_CN</language>
        <license>MIT OR Apache-2.0</license>
        <author>WayLan Contributors</author>
        <icon>input-keyboard</icon>
        <layout>us</layout>
        <rank>99</rank>
        <symbol>&#x6F9C;</symbol>
        <icon_prop_key>InputMode</icon_prop_key>
        <setup>{} settings</setup>
        <version>{}</version>
        <textdomain>waylan</textdomain>
    </engine>
</engines>
"#,
        exe_path, version
    )
}

/// 确保微澜组件已写入用户级 IBus 目录并同步至 GNOME 输入源列表
pub fn ensure_ibus_and_gnome_integration(exe_path: &str) {
    let Some(home) = env_var_non_empty("HOME") else {
        return;
    };
    let home_path = PathBuf::from(&home);
    let comp_dir = home_path.join(".local/share/ibus/component");
    if let Err(e) = fs::create_dir_all(&comp_dir) {
        warn!("[微澜 IBus] 创建用户组件目录 {:?} 失败: {}", comp_dir, e);
        return;
    }

    let xml_path = comp_dir.join("waylan.xml");
    let xml_content = format_ibus_component_xml(exe_path, env!("CARGO_PKG_VERSION"));
    let needs_write = fs::read_to_string(&xml_path)
        .map(|old| old != xml_content)
        .unwrap_or(true);

    if needs_write {
        if let Err(e) = fs::write(&xml_path, &xml_content) {
            warn!("[微澜 IBus] 写入组件描述文件 {:?} 失败: {}", xml_path, e);
            return;
        }
        info!("[微澜 IBus] 已更新用户级 IBus 组件描述文件: {:?}", xml_path);
        rebuild_user_ibus_cache(&comp_dir);
    }

    ensure_environment_d_config(&home_path, &comp_dir);
    ensure_gnome_input_source_registered();
}

/// 调用 `ibus write-cache` 将用户级组件编译进 `~/.cache/ibus/bus/registry`
fn rebuild_user_ibus_cache(comp_dir: &Path) {
    let comp_path_env = format!("/usr/share/ibus/component:{}", comp_dir.display());
    match Command::new("ibus")
        .arg("write-cache")
        .env("IBUS_COMPONENT_PATH", &comp_path_env)
        .output()
    {
        Ok(out) if out.status.success() => {
            info!("[微澜 IBus] 已成功更新 ~/.cache/ibus/bus/registry 组件缓存");
        }
        Ok(out) => {
            debug!(
                "[微澜 IBus] ibus write-cache 返回状态: {:?}",
                out.status.code()
            );
        }
        Err(e) => {
            debug!("[微澜 IBus] 调用 ibus write-cache 失败: {}", e);
        }
    }
}

/// 写入 `~/.config/environment.d/90-waylan-ibus.conf` 确保图形会话启动时自动包含用户级组件路径
fn ensure_environment_d_config(home_path: &Path, comp_dir: &Path) {
    let env_dir = home_path.join(".config/environment.d");
    if fs::create_dir_all(&env_dir).is_err() {
        return;
    }
    let conf_path = env_dir.join("90-waylan-ibus.conf");
    let expected = format!(
        "IBUS_COMPONENT_PATH=/usr/share/ibus/component:{}\n",
        comp_dir.display()
    );
    let needs_write = fs::read_to_string(&conf_path)
        .map(|old| old != expected)
        .unwrap_or(true);
    if needs_write {
        let _ = fs::write(&conf_path, expected);
    }
}

/// 检查并在 GNOME `org.gnome.desktop.input-sources` 中将 `('ibus', 'waylan')` 注册并置顶为首选输入源
pub fn ensure_gnome_input_source_registered() {
    update_gsettings_source_key("sources");
    update_gsettings_source_key("mru-sources");
}

/// 读取并更新指定的 GSettings 输入源键（将微澜置顶）
fn update_gsettings_source_key(key: &str) {
    let Ok(out) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.input-sources", key])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }

    let current = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if let Some(updated) = prioritize_waylan_in_gsettings_sources(&current) {
        if let Ok(st) = Command::new("gsettings")
            .args(["set", "org.gnome.desktop.input-sources", key, &updated])
            .status()
        {
            if st.success() {
                info!(
                    "[微澜 IBus] 已将 ('ibus', 'waylan') 置顶同步至 GNOME 系统 {} 列表: {}",
                    key, updated
                );
            }
        }
    }
}

/// 从 GVariant 元组数组字符串中解析提取所有的输入源项 `('type', 'id')`
fn parse_gsettings_source_tuples(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    let bracket_content = if let (Some(start), Some(end)) = (trimmed.find('['), trimmed.rfind(']'))
    {
        if start < end {
            &trimmed[start + 1..end]
        } else {
            ""
        }
    } else {
        ""
    };

    let mut items = Vec::new();
    let mut in_tuple = false;
    let mut current = String::new();
    for ch in bracket_content.chars() {
        if ch == '(' {
            in_tuple = true;
            current.clear();
            current.push(ch);
        } else if ch == ')' {
            if in_tuple {
                current.push(ch);
                items.push(current.clone());
                current.clear();
                in_tuple = false;
            }
        } else if in_tuple {
            current.push(ch);
        }
    }
    items
}

/// 在 `gsettings` 输入源列表字符串中将 `('ibus', 'waylan')` 置顶为首选输入源
///
/// 若微澜已在首位，返回 `None`；
/// 若微澜不在首位（或不在列表中），返回重新排序后的新字符串，将微澜置于首位并保持其余项的原有顺序。
pub fn prioritize_waylan_in_gsettings_sources(raw: &str) -> Option<String> {
    const WAYLAN_ENTRY: &str = "('ibus', 'waylan')";
    let items = parse_gsettings_source_tuples(raw);

    // 若列表首项已经是微澜，无需变更
    if let Some(first) = items.first() {
        if first.trim() == WAYLAN_ENTRY {
            return None;
        }
    }

    let mut new_items = Vec::with_capacity(items.len().saturating_add(1));
    new_items.push(WAYLAN_ENTRY.to_string());
    for item in items {
        let trimmed = item.trim();
        if trimmed != WAYLAN_ENTRY {
            new_items.push(trimmed.to_string());
        }
    }

    Some(format!("[{}]", new_items.join(", ")))
}

#[cfg(test)]
mod tests {
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

        // 空列表情况
        assert_eq!(
            prioritize_waylan_in_gsettings_sources("@a(ss) []").as_deref(),
            Some("[('ibus', 'waylan')]")
        );

        // 微澜已在首位时，无需变更返回 None
        assert!(
            prioritize_waylan_in_gsettings_sources("[('ibus', 'waylan'), ('xkb', 'cn')]").is_none()
        );
        assert!(prioritize_waylan_in_gsettings_sources("[('ibus', 'waylan')]").is_none());
    }

    #[test]
    fn test_format_ibus_component_and_engines_xml() {
        let comp_xml = format_ibus_component_xml("/usr/bin/waylan", "0.1.0");
        assert!(comp_xml.contains("<name>org.freedesktop.IBus.Waylan</name>"));
        assert!(comp_xml.contains("<exec>/usr/bin/waylan --ibus</exec>"));

        let eng_xml = format_ibus_engines_xml("/usr/bin/waylan", "0.1.0");
        assert!(eng_xml.contains("<name>waylan</name>"));
        assert!(eng_xml.contains("<setup>/usr/bin/waylan settings</setup>"));
    }
}
