//! 通用 GSettings 输入源列表解析与置顶原语
//!
//! # 设计原理
//! - **实现初衷**：封装 GNOME `org.gnome.desktop.input-sources` 的 GVariant 元组数组解析与排序写回。
//! - **核心优势**：输入源条目与回退 XKB 项均通过参数注入，底层库不硬编码具体输入法引擎名。
//! - **代价与局限**：依赖 `gsettings` 命令行工具与 GNOME schema。

use log::info;

use crate::process::{safe_gsettings_get, safe_gsettings_set, DEFAULT_COMMAND_TIMEOUT};

/// GNOME 输入源 GSettings Schema 名称
const INPUT_SOURCES_SCHEMA: &str = "org.gnome.desktop.input-sources";

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

/// 在 `gsettings` 输入源列表字符串中将目标输入源置顶，并确保存在兜底 XKB 项
///
/// 若目标项已在首位且存在 `('xkb', ...)`，返回 `None`；
/// 否则返回重新排序后的 GVariant 数组字符串。
pub fn prioritize_source_entry(
    raw: &str,
    target_entry: &str,
    fallback_xkb_entry: &str,
) -> Option<String> {
    let items = parse_gsettings_source_tuples(raw);
    let has_xkb = items.iter().any(|i| i.trim().starts_with("('xkb',"));

    if let Some(first) = items.first() {
        if first.trim() == target_entry && has_xkb {
            return None;
        }
    }

    let mut new_items = Vec::with_capacity(items.len().saturating_add(2));
    new_items.push(target_entry.to_string());
    for item in items {
        let trimmed = item.trim();
        if trimmed != target_entry {
            new_items.push(trimmed.to_string());
        }
    }
    if !has_xkb {
        new_items.push(fallback_xkb_entry.to_string());
    }

    Some(format!("[{}]", new_items.join(", ")))
}

/// 读取并更新指定的 GSettings 输入源键（将目标输入源置顶）
fn update_gsettings_source_key(key: &str, target_entry: &str, fallback_xkb_entry: &str) {
    let Some(current) = safe_gsettings_get(INPUT_SOURCES_SCHEMA, key, DEFAULT_COMMAND_TIMEOUT)
    else {
        return;
    };

    if let Some(updated) = prioritize_source_entry(&current, target_entry, fallback_xkb_entry) {
        if safe_gsettings_set(INPUT_SOURCES_SCHEMA, key, &updated, DEFAULT_COMMAND_TIMEOUT) {
            info!(
                "[桌面集成] 已将 {} 置顶同步至 GNOME 系统 {} 列表: {}",
                target_entry, key, updated
            );
        }
    }
}

/// 将指定输入源设为 GNOME 独占单输入源
pub fn set_exclusive_input_source(source_entry: &str) {
    let exclusive_sources = format!("[{}]", source_entry);
    for key in ["sources", "mru-sources"] {
        safe_gsettings_set(
            INPUT_SOURCES_SCHEMA,
            key,
            &exclusive_sources,
            DEFAULT_COMMAND_TIMEOUT,
        );
    }
    safe_gsettings_set(
        INPUT_SOURCES_SCHEMA,
        "current",
        "0",
        DEFAULT_COMMAND_TIMEOUT,
    );
    info!(
        "[桌面集成] 已将 {} 设为 GNOME 独占单输入源: {}",
        source_entry, exclusive_sources
    );
}

/// 确保指定输入源在 GNOME 输入源列表中注册并置顶（支持独占模式与共存置顶模式）
pub fn ensure_input_source_prioritized(
    source_entry: &str,
    fallback_xkb_entry: &str,
    exclusive: bool,
) {
    if exclusive {
        set_exclusive_input_source(source_entry);
    } else {
        update_gsettings_source_key("sources", source_entry, fallback_xkb_entry);
        update_gsettings_source_key("mru-sources", source_entry, fallback_xkb_entry);
        safe_gsettings_set(
            INPUT_SOURCES_SCHEMA,
            "current",
            "0",
            DEFAULT_COMMAND_TIMEOUT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prioritize_source_entry() {
        const ENTRY: &str = "('ibus', 'waylan')";
        const FALLBACK: &str = "('xkb', 'us')";

        assert_eq!(
            prioritize_source_entry(
                "[('xkb', 'cn'), ('ibus', 'libpinyin'), ('ibus', 'waylan')]",
                ENTRY,
                FALLBACK,
            )
            .as_deref(),
            Some("[('ibus', 'waylan'), ('xkb', 'cn'), ('ibus', 'libpinyin')]")
        );

        assert_eq!(
            prioritize_source_entry("[('xkb', 'cn'), ('ibus', 'libpinyin')]", ENTRY, FALLBACK,)
                .as_deref(),
            Some("[('ibus', 'waylan'), ('xkb', 'cn'), ('ibus', 'libpinyin')]")
        );

        assert_eq!(
            prioritize_source_entry("@a(ss) []", ENTRY, FALLBACK).as_deref(),
            Some("[('ibus', 'waylan'), ('xkb', 'us')]")
        );

        assert!(
            prioritize_source_entry("[('ibus', 'waylan'), ('xkb', 'cn')]", ENTRY, FALLBACK)
                .is_none()
        );

        assert_eq!(
            prioritize_source_entry("[('ibus', 'waylan')]", ENTRY, FALLBACK).as_deref(),
            Some("[('ibus', 'waylan'), ('xkb', 'us')]")
        );
    }
}
