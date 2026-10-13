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

/// 检查并在 GNOME `org.gnome.desktop.input-sources` 中注册并激活 `('ibus', 'waylan')`
pub fn ensure_gnome_input_source_registered() {
    let Ok(out) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.input-sources", "sources"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }

    let current = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if let Some(updated) = append_waylan_to_gsettings_sources(&current) {
        if let Ok(st) = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.input-sources",
                "sources",
                &updated,
            ])
            .status()
        {
            if st.success() {
                info!(
                    "[微澜 IBus] 已将 ('ibus', 'waylan') 合并至 GNOME 系统输入源列表: {}",
                    updated
                );
            }
        }
    }
}

/// 在 `gsettings` 输入源列表字符串中追加 `('ibus', 'waylan')`（若已存在则返回 `None`）
pub fn append_waylan_to_gsettings_sources(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.contains("('ibus', 'waylan')") {
        return None;
    }
    if trimmed == "@a(ss) []" || trimmed == "[]" {
        return Some("[('ibus', 'waylan')]".to_string());
    }
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if inner.is_empty() {
            return Some("[('ibus', 'waylan')]".to_string());
        }
        return Some(format!("[{}, ('ibus', 'waylan')]", inner));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_append_waylan_to_gsettings_sources() {
        assert_eq!(
            append_waylan_to_gsettings_sources("[('xkb', 'cn'), ('ibus', 'libpinyin')]").as_deref(),
            Some("[('xkb', 'cn'), ('ibus', 'libpinyin'), ('ibus', 'waylan')]")
        );
        assert_eq!(
            append_waylan_to_gsettings_sources("@a(ss) []").as_deref(),
            Some("[('ibus', 'waylan')]")
        );
        assert!(
            append_waylan_to_gsettings_sources("[('xkb', 'cn'), ('ibus', 'waylan')]").is_none()
        );
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
