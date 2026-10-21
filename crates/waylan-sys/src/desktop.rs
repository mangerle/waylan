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

/// 系统级 GNOME Shell 扩展目录（由发行版包管理器负责写入）
const GNOME_SYSTEM_EXTENSIONS_DIR: &str = "/usr/share/gnome-shell/extensions";

/// 扩展元数据文件读取体积上限（metadata.json 为数百字节量级，超限视为异常文件拒绝读入）
const MAX_EXTENSION_METADATA_BYTES: u64 = 64 * 1024;

/// 定位扩展源文件目录：兼容开发仓库布局（`<repo>/assets/gnome-extension`）与
/// 历史打包布局（`<share>/gnome-extension`），两者均不存在时返回 `None`
fn locate_extension_src_dir(assets: &str) -> Option<PathBuf> {
    let assets_dir = Path::new(assets);
    let candidates = [
        assets_dir.join("gnome-extension"),
        assets_dir
            .parent()
            .unwrap_or(assets_dir)
            .join("gnome-extension"),
    ];
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// 从扩展目录的 `metadata.json` 中读取整数 `version` 字段
///
/// 仅做定向扫描而不引入 JSON 解析依赖：该字段由本项目自行维护，格式稳定，
/// 且本 crate 定位为无应用层依赖的底层系统抽象。
fn read_extension_version(ext_dir: &Path) -> Option<u64> {
    let meta_path = ext_dir.join("metadata.json");
    let size = fs::metadata(&meta_path).ok()?.len();
    if size == 0 || size > MAX_EXTENSION_METADATA_BYTES {
        return None;
    }
    let text = fs::read_to_string(&meta_path).ok()?;
    let after_key = text.split_once("\"version\"")?.1;
    let digits = after_key
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse::<u64>().ok()
}

/// 判定用户目录副本是否应让位给系统目录副本
///
/// 版本读取失败时保守返回 `false`：宁可继续加载旧的用户副本，也不能把用户手工安装的
/// 扩展删除（那会让输入法的候选框渲染彻底消失）。
#[must_use]
fn should_retire_user_copy(user: Option<u64>, system: Option<u64>) -> bool {
    matches!((user, system), (Some(u), Some(s)) if u < s)
}

/// 若用户目录副本版本落后于系统目录副本，则直接删除该副本让位
///
/// GNOME 对同一 uuid 优先加载用户目录副本，仅刷新系统副本无法让新版生效；
/// 只有两侧版本均可判读且用户副本严格落后时才删除，版本不可判读时保留用户副本。
fn retire_stale_user_copy(user_dir: &Path, system_dir: &Path) {
    if !user_dir.is_dir() {
        return;
    }

    let user_version = read_extension_version(user_dir);
    let system_version = read_extension_version(system_dir);
    if !should_retire_user_copy(user_version, system_version) {
        debug!(
            "[桌面集成] 用户目录扩展副本 (版本 {:?}) 无需让位于系统副本 (版本 {:?})",
            user_version, system_version
        );
        return;
    }

    match fs::remove_dir_all(user_dir) {
        Ok(()) => warn!(
            "[桌面集成] 用户目录扩展副本 (版本 {:?}) 落后于系统副本 (版本 {:?})，已删除；\
             重启 GNOME Shell 后将加载系统版本扩展。",
            user_version, system_version
        ),
        Err(e) => warn!(
            "[桌面集成] 删除陈旧用户扩展副本 {:?} 失败: {}，扩展行为仍将是旧版本",
            user_dir, e
        ),
    }
}

/// 从资源目录把扩展文件同步进指定用户目录（仅在系统目录缺失时作为回退通道使用）
fn install_extension_to_user_dir(target: &Path) {
    let Some(assets) = detect_default_assets_path() else {
        return;
    };
    let Some(src_dir) = locate_extension_src_dir(&assets) else {
        // 既无系统目录副本又找不到源码资源，属安装不完整的异常环境，需可检索的告警。
        warn!(
            "[桌面集成] 未找到微澜 GNOME 扩展源：系统目录缺失且资源路径 {:?} 下无 gnome-extension 目录",
            assets
        );
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

/// 保障 GNOME Shell 扩展文件就绪：系统目录可用时让位给它，否则回退安装到用户目录
///
/// # 设计原理
/// - **实现初衷**：扩展文件的安装权属于包管理器（deb 直接装到 `/usr/share/gnome-shell/extensions`）。
///   原实现无条件把扩展复制进 `~/.local/share/gnome-shell/extensions`，而 GNOME 对同一 uuid
///   **优先加载用户目录副本**，于是包升级只刷新系统副本、用户副本永久遮蔽新版，表现为
///   "升级后扩展行为仍是旧的"，且复制源路径在打包布局下本就解析不到，连刷新都做不到。
/// - **核心优势**：打包环境不再产生双副本；历史遗留的落后副本在启动期自动删除让位，
///   家目录不残留无主文件；源码运行环境仍保留回退安装通道。
/// - **代价与局限**：让位后需重启 GNOME Shell 才会加载系统副本（扩展仅在 shell 启动时枚举一次）；
///   版本无法判读时保守保留用户副本，不冒险删除。
pub fn ensure_gnome_hide_indicator_extension_files(extension_uuid: &str) {
    let Some(home) = env_var_non_empty("HOME") else {
        return;
    };
    let user_dir = PathBuf::from(&home)
        .join(".local/share/gnome-shell/extensions")
        .join(extension_uuid);
    let system_dir = Path::new(GNOME_SYSTEM_EXTENSIONS_DIR).join(extension_uuid);

    if system_dir.is_dir() {
        retire_stale_user_copy(&user_dir, &system_dir);
        return;
    }
    install_extension_to_user_dir(&user_dir);
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
    ensure_gnome_hide_indicator_extension_files(extension_uuid);
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

    /// 在临时目录下构造一个带指定 metadata 内容的扩展目录
    fn make_extension_dir(root: &Path, uuid: &str, metadata: &str) -> PathBuf {
        let dir = root.join(uuid);
        fs::create_dir_all(&dir).expect("创建临时扩展目录失败");
        fs::write(dir.join("metadata.json"), metadata).expect("写入 metadata.json 失败");
        dir
    }

    #[test]
    fn test_read_extension_version_parses_metadata_version() {
        let tmp = tempfile::tempdir().expect("创建临时目录失败");
        let dir = make_extension_dir(
            tmp.path(),
            "x@y.org",
            "{\n  \"uuid\": \"x@y.org\",\n  \"version\": 3\n}\n",
        );
        assert_eq!(read_extension_version(&dir), Some(3));
    }

    #[test]
    fn test_read_extension_version_rejects_missing_and_malformed() {
        let tmp = tempfile::tempdir().expect("创建临时目录失败");

        // 目录不存在：读取失败必须返回 None 而非 panic
        assert_eq!(
            read_extension_version(&tmp.path().join("absent@x.org")),
            None
        );

        // 无 version 字段与非数字取值：均视为不可判读
        let no_field = make_extension_dir(tmp.path(), "nofield@x.org", "{\"name\": \"WayLan\"}");
        assert_eq!(read_extension_version(&no_field), None);
        let bad_value = make_extension_dir(tmp.path(), "bad@x.org", "{\"version\": \"three\"}");
        assert_eq!(read_extension_version(&bad_value), None);
    }

    #[test]
    fn test_read_extension_version_rejects_oversized_metadata() {
        let tmp = tempfile::tempdir().expect("创建临时目录失败");
        let dir = tmp.path().join("big@x.org");
        fs::create_dir_all(&dir).expect("创建临时扩展目录失败");
        // 用户可写的家目录文件，超限必须拒读以防被撑爆内存
        let padded = format!(
            "{{\"version\": 7, \"pad\": \"{}\"}}",
            "a".repeat(MAX_EXTENSION_METADATA_BYTES as usize)
        );
        fs::write(dir.join("metadata.json"), padded).expect("写入超大 metadata 失败");
        assert_eq!(read_extension_version(&dir), None);
    }

    #[test]
    fn test_should_retire_user_copy_only_when_strictly_older() {
        assert!(
            should_retire_user_copy(Some(1), Some(3)),
            "用户副本版本更低时应让位于系统副本"
        );
        assert!(
            !should_retire_user_copy(Some(3), Some(3)),
            "版本相同不让位，避免每次启动反复删除"
        );
        assert!(
            !should_retire_user_copy(Some(5), Some(3)),
            "用户副本更新时不得被系统旧副本覆盖"
        );
        assert!(
            !should_retire_user_copy(None, Some(3)),
            "版本不可判读时保守保留用户副本"
        );
        assert!(
            !should_retire_user_copy(Some(1), None),
            "系统副本版本不可判读时不让位"
        );
    }

    #[test]
    fn test_retire_stale_user_copy_deletes_older_user_copy() {
        let tmp = tempfile::tempdir().expect("创建临时目录失败");
        let user_root = tmp.path().join("user-extensions");
        let system_root = tmp.path().join("system-extensions");
        let user_dir = make_extension_dir(&user_root, "x@y.org", "{\"version\": 1}");
        let system_dir = make_extension_dir(&system_root, "x@y.org", "{\"version\": 3}");

        retire_stale_user_copy(&user_dir, &system_dir);

        assert!(!user_dir.exists(), "落后的用户副本应被直接删除");
        assert!(
            system_dir.join("metadata.json").is_file(),
            "系统副本不得被触碰"
        );
    }

    #[test]
    fn test_retire_stale_user_copy_keeps_not_older_or_unknown() {
        let tmp = tempfile::tempdir().expect("创建临时目录失败");

        // 用户副本更新：必须保留，不能被系统旧副本反向覆盖
        let newer_user_root = tmp.path().join("newer-user");
        let newer_user = make_extension_dir(&newer_user_root, "x@y.org", "{\"version\": 5}");
        let old_system = make_extension_dir(
            &tmp.path().join("old-system"),
            "x@y.org",
            "{\"version\": 3}",
        );
        retire_stale_user_copy(&newer_user, &old_system);
        assert!(newer_user.exists(), "版本更新的用户副本不得被删除");

        // 版本不可判读：保守保留用户副本
        let unknown_user_root = tmp.path().join("unknown-user");
        let unknown_user =
            make_extension_dir(&unknown_user_root, "x@y.org", "{\"name\": \"WayLan\"}");
        let known_system = make_extension_dir(
            &tmp.path().join("known-system"),
            "x@y.org",
            "{\"version\": 3}",
        );
        retire_stale_user_copy(&unknown_user, &known_system);
        assert!(unknown_user.exists(), "版本不可判读时不得删除用户副本");
    }
}
