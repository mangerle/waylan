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

/// 解析微澜 IBus 引擎注册所使用的图标路径（优先绝对路径以确保 GNOME 原生顶栏准确渲染）
fn resolve_engine_icon_path() -> String {
    if let Some(assets) = crate::tray::item::detect_default_assets_path() {
        crate::tray::item::resolve_icon_path_or_name(&assets, "waylan")
    } else {
        "waylan".to_string()
    }
}

/// 自动将微澜矢量图标同步注册至用户级 XDG 图标目录 `~/.local/share/icons/hicolor/scalable/apps/`
fn ensure_user_icons_registered(home_path: &Path) {
    let Some(assets_path) = crate::tray::item::detect_default_assets_path() else {
        return;
    };
    let assets_dir = Path::new(&assets_path);
    let target_dir = home_path.join(".local/share/icons/hicolor/scalable/apps");
    if fs::create_dir_all(&target_dir).is_err() {
        return;
    }

    let icons = [
        "waylan.svg",
        "tray-zh.svg",
        "tray-en.svg",
        "tray-en-caps.svg",
    ];
    let mut updated = false;
    for icon_name in icons {
        let src = assets_dir.join(icon_name);
        let dst = target_dir.join(icon_name);
        if src.is_file() {
            let needs_copy = match (fs::metadata(&src), fs::metadata(&dst)) {
                (Ok(s_meta), Ok(d_meta)) => s_meta.len() != d_meta.len(),
                (Ok(_), Err(_)) => true,
                _ => false,
            };
            if needs_copy && fs::copy(&src, &dst).is_ok() {
                updated = true;
            }
        }
    }

    if updated {
        info!(
            "[微澜 IBus] 已自动同步微澜矢量图标至用户图标目录: {:?}",
            target_dir
        );
        let _ = Command::new("gtk-update-icon-cache")
            .arg("-f")
            .arg("-t")
            .arg(home_path.join(".local/share/icons/hicolor"))
            .output();
    }
}

/// 生成完整的 IBus `<component>` XML 文本
pub fn format_ibus_component_xml(exe_path: &str, version: &str) -> String {
    let icon_path = resolve_engine_icon_path();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<component>
    <name>org.freedesktop.IBus.Waylan</name>
    <description>WayLan Chinese Input Method Component</description>
    <exec>{exe} --ibus</exec>
    <version>{ver}</version>
    <author>wangle</author>
    <license>Apache-2.0</license>
    <homepage>https://github.com/mangerle/waylan</homepage>
    <textdomain>waylan</textdomain>
    <engines>
        <engine>
            <name>waylan</name>
            <longname>微澜拼音</longname>
            <description>微澜 (WayLan) - 现代轻量纯 Rust 中文拼音输入法</description>
            <language>zh_CN</language>
            <license>Apache-2.0</license>
            <author>wangle</author>
            <icon>{icon}</icon>
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
        ver = version,
        icon = icon_path,
    )
}

/// 生成符合 IBus `<engines>` 规范的 XML 描述文本（响应 `waylan --xml`）
pub fn format_ibus_engines_xml(exe_path: &str, version: &str) -> String {
    let icon_path = resolve_engine_icon_path();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<engines>
    <engine>
        <name>waylan</name>
        <longname>微澜拼音</longname>
        <description>微澜 (WayLan) - 现代轻量纯 Rust 中文拼音输入法</description>
        <language>zh_CN</language>
        <license>MIT OR Apache-2.0</license>
        <author>WayLan Contributors</author>
        <icon>{}</icon>
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
        icon_path, exe_path, version
    )
}

/// 确保微澜组件注册就绪并同步置顶至 GNOME 输入源列表
pub fn ensure_ibus_and_gnome_integration(exe_path: &str, exclusive: bool) {
    let Some(home) = env_var_non_empty("HOME") else {
        return;
    };
    let home_path = PathBuf::from(&home);
    ensure_user_icons_registered(&home_path);

    let sys_xml = Path::new("/usr/share/ibus/component/waylan.xml");
    let comp_dir = home_path.join(".local/share/ibus/component");
    let user_xml = comp_dir.join("waylan.xml");
    let env_conf = home_path.join(".config/environment.d/90-waylan-ibus.conf");

    if sys_xml.exists() {
        // 系统级组件已存在（deb 包安装），清理用户目录冗余文件以防 ibus 产生重复注册冲突
        let mut cleaned = false;
        if user_xml.exists() {
            let _ = fs::remove_file(&user_xml);
            cleaned = true;
        }
        if env_conf.exists() {
            let _ = fs::remove_file(&env_conf);
            cleaned = true;
        }
        if cleaned {
            info!("[微澜 IBus] 系统级组件已就绪，已清理用户目录冗余描述并刷新缓存。");
            let _ = Command::new("ibus").arg("write-cache").output();
        }
    } else {
        // 系统级组件不存在（源码开发/测试环境），写入用户级组件并配置 environment.d
        if let Err(e) = fs::create_dir_all(&comp_dir) {
            warn!("[微澜 IBus] 创建用户组件目录 {:?} 失败: {}", comp_dir, e);
            return;
        }

        let xml_content = format_ibus_component_xml(exe_path, env!("CARGO_PKG_VERSION"));
        let needs_write = fs::read_to_string(&user_xml)
            .map(|old| old != xml_content)
            .unwrap_or(true);

        if needs_write {
            if let Err(e) = fs::write(&user_xml, &xml_content) {
                warn!("[微澜 IBus] 写入组件描述文件 {:?} 失败: {}", user_xml, e);
                return;
            }
            info!("[微澜 IBus] 已更新用户级 IBus 组件描述文件: {:?}", user_xml);
            rebuild_user_ibus_cache(&comp_dir);
        }

        ensure_environment_d_config(&home_path, &comp_dir);
    }

    ensure_gnome_input_source_registered(exclusive);
    sync_gnome_hide_indicator_extension(exclusive);
}

/// 同步 GNOME Shell 原生输入法指示器隐藏扩展的启用/禁用状态
pub fn sync_gnome_hide_indicator_extension(hide_native: bool) {
    let is_gnome = env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false);
    if !is_gnome {
        return;
    }

    let extension_uuid = "waylan-hide-input-source@waylan.org";
    let sys_dir = Path::new("/usr/share/gnome-shell/extensions").join(extension_uuid);
    let user_dir = env_var_non_empty("HOME").map(|h| {
        PathBuf::from(h)
            .join(".local/share/gnome-shell/extensions")
            .join(extension_uuid)
    });
    let exists = sys_dir.is_dir() || user_dir.as_ref().is_some_and(|d| d.is_dir());

    if !exists {
        if let Some(assets) = crate::tray::item::detect_default_assets_path() {
            let src_dir = Path::new(&assets)
                .parent()
                .unwrap_or(Path::new(&assets))
                .join("gnome-extension");
            if src_dir.is_dir() {
                if let Some(ref target) = user_dir {
                    if fs::create_dir_all(target).is_ok() {
                        let _ =
                            fs::copy(src_dir.join("metadata.json"), target.join("metadata.json"));
                        let _ = fs::copy(src_dir.join("extension.js"), target.join("extension.js"));
                        info!(
                            "[微澜 IBus] 已自动部署 GNOME 隐藏输入法指示器扩展至用户目录: {:?}",
                            target
                        );
                    }
                }
            }
        }
    }

    let action = if hide_native { "enable" } else { "disable" };
    update_gsettings_enabled_extensions(extension_uuid, hide_native);
    match Command::new("gnome-extensions")
        .args([action, extension_uuid])
        .output()
    {
        Ok(out) if out.status.success() => {
            info!(
                "[微澜 IBus] 已成功{} GNOME 隐藏原生指示器扩展 ({})",
                if hide_native { "启用" } else { "禁用" },
                extension_uuid
            );
        }
        Ok(out) => {
            debug!(
                "[微澜 IBus] gnome-extensions {} 返回状态: {:?}",
                action,
                out.status.code()
            );
        }
        Err(e) => {
            debug!("[微澜 IBus] 调用 gnome-extensions 失败: {}", e);
        }
    }
}

/// 同步更新 GNOME GSettings enabled-extensions 列表
fn update_gsettings_enabled_extensions(uuid: &str, enable: bool) {
    let Ok(out) = Command::new("gsettings")
        .args(["get", "org.gnome.shell", "enabled-extensions"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut list: Vec<String> = raw
        .trim_matches(|c| c == '[' || c == ']' || c == ' ')
        .split(',')
        .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut changed = false;
    if enable {
        if !list.iter().any(|item| item == uuid) {
            list.push(uuid.to_string());
            changed = true;
        }
    } else if list.iter().any(|item| item == uuid) {
        list.retain(|item| item != uuid);
        changed = true;
    }

    if changed {
        let formatted = format!(
            "[{}]",
            list.iter()
                .map(|s| format!("'{}'", s))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let _ = Command::new("gsettings")
            .args(["set", "org.gnome.shell", "enabled-extensions", &formatted])
            .output();
        info!(
            "[微澜 IBus] 已同步更新 GNOME enabled-extensions: {}",
            formatted
        );
    }
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
/// 若 `exclusive` 为 true，则将微澜设为系统独占首选输入源（保留基础 XKB 映射以符合 GNOME 规范）
pub fn ensure_gnome_input_source_registered(exclusive: bool) {
    if exclusive {
        set_waylan_as_exclusive_input_source();
    } else {
        update_gsettings_source_key("sources");
        update_gsettings_source_key("mru-sources");
    }
    // 同步将 GNOME 当前输入源索引切换至首项（微澜）
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.desktop.input-sources", "current", "0"])
        .output();
    // 确保 IBus 全局引擎激活为微澜
    let _ = Command::new("ibus").args(["engine", "waylan"]).output();
}

/// 将微澜设为 GNOME 首选输入源并保留底层必须的 XKB 键盘布局
///
/// GNOME 规范严格要求必须存在至少一项底层 XKB 键盘布局以处理基础硬件按键映射。
/// 独占托盘模式将微澜设为首位并保留兜底英文 XKB，原生白色指示器则交由微澜 GNOME 扩展彻底隐藏。
pub fn set_waylan_as_exclusive_input_source() {
    let exclusive_sources = "[('ibus', 'waylan'), ('xkb', 'us')]";

    for key in ["sources", "mru-sources"] {
        let _ = Command::new("gsettings")
            .args([
                "set",
                "org.gnome.desktop.input-sources",
                key,
                exclusive_sources,
            ])
            .output();
    }
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.desktop.input-sources", "current", "0"])
        .output();
    let _ = Command::new("ibus").args(["engine", "waylan"]).output();
    info!(
        "[微澜 IBus] 已将微澜设为 GNOME 首选输入源并保留 XKB 硬件按键基础映射: {}",
        exclusive_sources
    );
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

    let has_xkb = items.iter().any(|i| i.trim().starts_with("('xkb',"));

    // 若列表首项已经是微澜且包含底层 XKB 映射，无需变更
    if let Some(first) = items.first() {
        if first.trim() == WAYLAN_ENTRY && has_xkb {
            return None;
        }
    }

    let mut new_items = Vec::with_capacity(items.len().saturating_add(2));
    new_items.push(WAYLAN_ENTRY.to_string());
    for item in items {
        let trimmed = item.trim();
        if trimmed != WAYLAN_ENTRY {
            new_items.push(trimmed.to_string());
        }
    }
    // 确保列表内至少包含一项底层 XKB 映射（GNOME 强制要求）
    if !has_xkb {
        new_items.push("('xkb', 'us')".to_string());
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
        let comp_xml = format_ibus_component_xml("/usr/bin/waylan", "0.1.0");
        assert!(comp_xml.contains("<name>org.freedesktop.IBus.Waylan</name>"));
        assert!(comp_xml.contains("<exec>/usr/bin/waylan --ibus</exec>"));

        let eng_xml = format_ibus_engines_xml("/usr/bin/waylan", "0.1.0");
        assert!(eng_xml.contains("<name>waylan</name>"));
        assert!(eng_xml.contains("<setup>/usr/bin/waylan settings</setup>"));
    }
}
