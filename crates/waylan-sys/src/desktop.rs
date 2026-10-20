//! GNOME 桌面环境原生输入源与配套 Shell 扩展集成管理
//!
//! # 设计原理
//! - **实现初衷**：GNOME 顶栏输入源列表由 org.gnome.desktop.input-sources 控制。
//!   本模块负责将微澜置顶为首选输入源，并按 [`GnomeExtensionIntent`] 描述的两项职责
//!   （隐藏原生冗余指示器、渲染微澜自定义候选框）决定配套 Shell 扩展是否需要加载。
//! - **核心优势**：兼顾 GNOME 规范对底层 XKB 映射的强要求，同时保证微澜处于首选激活位置并保留系统原有输入源。
//! - **代价与局限**：依赖 gsettings 与 gnome-extensions 命令行工具，仅在 GNOME 桌面环境下生效。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use log::{debug, info, warn};

use crate::assets::detect_default_assets_path;
use crate::process::{
    run_command_with_timeout, safe_gsettings_get, safe_gsettings_set, DEFAULT_COMMAND_TIMEOUT,
};

/// 检查当前是否处于 GNOME 桌面环境
pub fn is_gnome_desktop() -> bool {
    env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false)
}

/// 读取非空环境变量
fn env_var_non_empty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// GNOME Shell 扩展 `waylan-hide-input-source` 的启用意图
///
/// # 设计原理
/// - **实现初衷**：该扩展身兼两职——隐藏顶栏原生输入源指示器，以及在微澜自定义候选框模式下
///   接管 IBus 候选弹窗渲染微澜胶囊视图。原实现仅用"独占输入源"一个布尔量决定整个扩展是否加载，
///   导致用户选择了微澜候选框但未开启独占输入源时，扩展根本不加载，全部候选框外观设置静默失效。
/// - **核心优势**：加载条件取两职并集，扩展内部再按各自独立配置分别生效，
///   界面上的"独占输入源"与"候选框模式"恢复互不牵连的语义。
/// - **代价与局限**：两职皆关时扩展仍会因配置需要而加载（仅剩配置监听开销）；
///   指示器隐藏从"随扩展加载无条件执行"改为"扩展内部按配置热切换"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GnomeExtensionIntent {
    /// 是否需要隐藏顶栏原生输入源指示器（对应独占输入源模式）
    pub hide_indicator: bool,
    /// 是否需要由扩展渲染微澜自定义候选框（对应候选框模式为微澜专属）
    pub custom_candidate_view: bool,
}

impl GnomeExtensionIntent {
    /// 扩展是否应当处于加载状态：两职责任一需要即须加载
    #[must_use]
    pub fn should_enable(self) -> bool {
        self.hide_indicator || self.custom_candidate_view
    }
}

/// 定位扩展源文件目录：兼容开发仓库布局（`<repo>/assets/gnome-extension`）与
/// 历史打包布局（`<share>/gnome-extension`），两者均不存在时返回 `None`
fn locate_extension_src_dir(assets: &str) -> Option<PathBuf> {
    let assets_dir = Path::new(assets);
    let candidates = [assets_dir.join("gnome-extension"), {
        assets_dir
            .parent()
            .unwrap_or(assets_dir)
            .join("gnome-extension")
    }];
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// 安装或更新 GNOME Shell 隐藏原生指示器扩展文件至用户扩展目录
pub fn install_gnome_hide_indicator_extension(extension_uuid: &str) {
    let user_dir = env_var_non_empty("HOME").map(|h| {
        PathBuf::from(h)
            .join(".local/share/gnome-shell/extensions")
            .join(extension_uuid)
    });

    let Some(assets) = detect_default_assets_path() else {
        return;
    };
    let Some(src_dir) = locate_extension_src_dir(&assets) else {
        // 扩展文件由系统包直接安装至 /usr/share/gnome-shell/extensions 时属正常情形，
        // 此处仅跳过用户目录同步；记为 debug 以便排查"改了扩展代码但界面没变"的问题。
        debug!(
            "[桌面集成] 未找到微澜 GNOME 扩展源目录 (基于资源路径 {:?})，跳过用户目录同步",
            assets
        );
        return;
    };
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
                warn!("[桌面集成] 同步 GNOME 扩展文件 {:?} 失败: {}", src_file, e);
            }
        }
    }
}

/// 通过 `gnome-extensions` CLI 与 `gsettings` 设置扩展的启用或禁用状态
pub fn set_gnome_extension_enabled(extension_uuid: &str, enable: bool) {
    let action = if enable { "enable" } else { "disable" };
    update_gsettings_enabled_extensions(extension_uuid, enable);
    let mut cmd = Command::new("gnome-extensions");
    cmd.args([action, extension_uuid]);
    match run_command_with_timeout(cmd, DEFAULT_COMMAND_TIMEOUT) {
        Ok(out) if out.status.success() => {
            info!(
                "[桌面集成] 已成功{} GNOME 隐藏原生指示器扩展 ({})",
                if enable { "启用" } else { "禁用" },
                extension_uuid
            );
        }
        Ok(out) => {
            debug!(
                "[桌面集成] gnome-extensions {} 返回状态: {:?}",
                action,
                out.status.code()
            );
        }
        Err(e) => {
            debug!("[桌面集成] 调用 gnome-extensions 失败或超时: {}", e);
        }
    }
}

/// 同步 GNOME Shell 扩展的加载状态：任一职责需要即加载，具体做什么由扩展内部按配置判定
pub fn sync_gnome_hide_indicator_extension(intent: GnomeExtensionIntent) {
    if !is_gnome_desktop() {
        return;
    }

    let extension_uuid = "waylan-hide-input-source@waylan.org";
    install_gnome_hide_indicator_extension(extension_uuid);
    set_gnome_extension_enabled(extension_uuid, intent.should_enable());
}

/// 同步更新 GNOME GSettings enabled-extensions 列表
fn update_gsettings_enabled_extensions(uuid: &str, enable: bool) {
    let Some(raw) = safe_gsettings_get(
        "org.gnome.shell",
        "enabled-extensions",
        DEFAULT_COMMAND_TIMEOUT,
    ) else {
        return;
    };
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
        safe_gsettings_set(
            "org.gnome.shell",
            "enabled-extensions",
            &formatted,
            DEFAULT_COMMAND_TIMEOUT,
        );
        info!(
            "[桌面集成] 已同步更新 GNOME enabled-extensions: {}",
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
        // 同步将 GNOME 当前输入源索引切换至首项（微澜）
        safe_gsettings_set(
            "org.gnome.desktop.input-sources",
            "current",
            "0",
            DEFAULT_COMMAND_TIMEOUT,
        );
    }
}

/// 将微澜设为 GNOME 首选输入源并保留底层必须的 XKB 键盘布局
///
/// GNOME 规范严格要求必须存在至少一项底层 XKB 键盘布局以处理基础硬件按键映射。
/// 独占托盘模式将微澜设为首位并保留兜底英文 XKB，原生白色指示器则交由微澜 GNOME 扩展彻底隐藏。
pub fn set_waylan_as_exclusive_input_source() {
    let exclusive_sources = "[('ibus', 'waylan'), ('xkb', 'us')]";

    for key in ["sources", "mru-sources"] {
        safe_gsettings_set(
            "org.gnome.desktop.input-sources",
            key,
            exclusive_sources,
            DEFAULT_COMMAND_TIMEOUT,
        );
    }
    safe_gsettings_set(
        "org.gnome.desktop.input-sources",
        "current",
        "0",
        DEFAULT_COMMAND_TIMEOUT,
    );
    info!(
        "[桌面集成] 已将微澜设为 GNOME 首选输入源并保留 XKB 硬件按键基础映射: {}",
        exclusive_sources
    );
}

/// 读取并更新指定的 GSettings 输入源键（将微澜置顶）
fn update_gsettings_source_key(key: &str) {
    let Some(current) = safe_gsettings_get(
        "org.gnome.desktop.input-sources",
        key,
        DEFAULT_COMMAND_TIMEOUT,
    ) else {
        return;
    };

    if let Some(updated) = prioritize_waylan_in_gsettings_sources(&current) {
        if safe_gsettings_set(
            "org.gnome.desktop.input-sources",
            key,
            &updated,
            DEFAULT_COMMAND_TIMEOUT,
        ) {
            info!(
                "[桌面集成] 已将 ('ibus', 'waylan') 置顶同步至 GNOME 系统 {} 列表: {}",
                key, updated
            );
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
    if !has_xkb {
        new_items.push("('xkb', 'us')".to_string());
    }

    Some(format!("[{}]", new_items.join(", ")))
}

/// 后台派发扩展加载状态对齐：仅同步扩展本身，不触碰输入源与当前引擎选择
///
/// 供守护进程启动自检使用——扩展可能因用户升级、在扩展管理器里手动关闭或首次安装而与
/// 配置脱节，而外部命令均带超时，必须离开主启动路径执行，否则会拖慢会话就绪。
pub fn sync_gnome_extension_state_async(intent: GnomeExtensionIntent) {
    if !is_gnome_desktop() {
        return;
    }

    let spawn_res = std::thread::Builder::new()
        .name("sync-gnome-extension".to_string())
        .spawn(move || sync_gnome_hide_indicator_extension(intent));

    if let Err(e) = spawn_res {
        warn!("[桌面集成 警告] 派发启动期扩展状态对齐线程失败: {}", e);
    }
}

/// 安全同步桌面环境输入源与扩展加载状态（后台异步派发执行，具备超时保护并保留用户原有输入源）
pub fn safe_sync_desktop_input_sources(intent: GnomeExtensionIntent) {
    if !is_gnome_desktop() {
        return;
    }

    let spawn_res = std::thread::Builder::new()
        .name("sync-desktop-mode".to_string())
        .spawn(move || {
            ensure_gnome_input_source_registered(intent.hide_indicator);
            sync_gnome_hide_indicator_extension(intent);

            let mut cmd = Command::new("ibus");
            cmd.args(["engine", "waylan"]);
            let _ = run_command_with_timeout(cmd, DEFAULT_COMMAND_TIMEOUT);
        });

    if let Err(e) = spawn_res {
        warn!("[桌面集成 警告] 派发桌面输入源同步线程失败: {}", e);
    }
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
}
