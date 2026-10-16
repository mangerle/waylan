//! GNOME 桌面环境原生输入源与隐藏指示器扩展集成管理
//!
//! # 设计原理
//! - **实现初衷**：GNOME 顶栏输入源列表由 org.gnome.desktop.input-sources 控制。
//!   本模块负责将微澜置顶为首选输入源，并在独占托盘模式下安装并启用 GNOME Shell 扩展以隐藏原生冗余指示器。
//! - **核心优势**：兼顾 GNOME 规范对底层 XKB 映射的强要求，同时保证微澜处于首选激活位置。
//! - **代价与局限**：依赖 gsettings 与 gnome-extensions 命令行工具，仅在 GNOME 桌面环境下生效。

use log::{debug, info, warn};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::env_var_non_empty;

/// 安装或更新 GNOME Shell 隐藏原生指示器扩展文件至用户扩展目录
pub(crate) fn install_gnome_hide_indicator_extension(extension_uuid: &str) {
    let user_dir = env_var_non_empty("HOME").map(|h| {
        PathBuf::from(h)
            .join(".local/share/gnome-shell/extensions")
            .join(extension_uuid)
    });

    let Some(assets) = crate::tray::item::detect_default_assets_path() else {
        return;
    };
    let src_dir = Path::new(&assets)
        .parent()
        .unwrap_or(Path::new(&assets))
        .join("gnome-extension");
    if !src_dir.is_dir() {
        return;
    }
    let Some(ref target) = user_dir else {
        return;
    };
    if fs::create_dir_all(target).is_err() {
        return;
    }

    for file_name in ["metadata.json", "extension.js", "stylesheet.css"] {
        let src_file = src_dir.join(file_name);
        let dst_file = target.join(file_name);
        if !src_file.is_file() {
            continue;
        }
        let needs_copy = match (fs::metadata(&src_file), fs::metadata(&dst_file)) {
            (Ok(s_meta), Ok(d_meta)) => s_meta.len() != d_meta.len(),
            (Ok(_), Err(_)) => true,
            _ => false,
        };
        if needs_copy {
            if let Err(e) = fs::copy(&src_file, &dst_file) {
                warn!(
                    "[微澜 IBus 警告] 同步 GNOME 扩展文件 {:?} 失败: {}",
                    src_file, e
                );
            }
        }
    }
}

/// 通过 `gnome-extensions` CLI 与 `gsettings` 设置扩展的启用或禁用状态
pub(crate) fn set_gnome_extension_enabled(extension_uuid: &str, enable: bool) {
    let action = if enable { "enable" } else { "disable" };
    update_gsettings_enabled_extensions(extension_uuid, enable);
    match Command::new("gnome-extensions")
        .args([action, extension_uuid])
        .output()
    {
        Ok(out) if out.status.success() => {
            info!(
                "[微澜 IBus] 已成功{} GNOME 隐藏原生指示器扩展 ({})",
                if enable { "启用" } else { "禁用" },
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

/// 同步 GNOME Shell 原生输入法指示器隐藏扩展的启用/禁用状态
pub fn sync_gnome_hide_indicator_extension(hide_native: bool) {
    let is_gnome = env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false);
    if !is_gnome {
        return;
    }

    let extension_uuid = "waylan-hide-input-source@waylan.org";
    install_gnome_hide_indicator_extension(extension_uuid);
    set_gnome_extension_enabled(extension_uuid, hide_native);
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
    }
    .trim();

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
