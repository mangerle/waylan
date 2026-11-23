//! 通用 GNOME Shell 扩展文件同步与启停管理原语
//!
//! # 设计原理
//! - **实现初衷**：提供无应用层硬编码的 GNOME Shell 扩展版本让位、回退安装与启用/禁用能力。
//! - **核心优势**：扩展 UUID 与资源子目录均由上层业务传入，`waylan-sys` 保持纯粹的系统抽象边界。
//! - **代价与局限**：依赖宿主机 `gnome-extensions` 与 `gsettings` 命令行工具。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use log::{debug, info, warn};

use super::env_var_non_empty;
use crate::assets::detect_default_assets_path;
use crate::process::{
    DEFAULT_COMMAND_TIMEOUT, run_command_with_timeout, safe_gsettings_get, safe_gsettings_set,
};

/// 系统级 GNOME Shell 扩展目录（由发行版包管理器负责写入）
const GNOME_SYSTEM_EXTENSIONS_DIR: &str = "/usr/share/gnome-shell/extensions";

/// 扩展元数据文件读取体积上限（metadata.json 为数百字节量级，超限视为异常文件拒绝读入）
const MAX_EXTENSION_METADATA_BYTES: u64 = 64 * 1024;

/// 定位扩展源文件目录：兼容开发仓库布局（`<repo>/assets/<subdir>`）与
/// 历史打包布局（`<share>/<subdir>`），两者均不存在时返回 `None`
fn locate_extension_src_dir(assets: &str, subdir: &str) -> Option<PathBuf> {
    let assets_dir = Path::new(assets);
    let candidates = [
        assets_dir.join(subdir),
        assets_dir.parent().unwrap_or(assets_dir).join(subdir),
    ];
    candidates.into_iter().find(|dir| dir.is_dir())
}

/// 从扩展目录的 `metadata.json` 中读取整数 `version` 字段
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

/// 判定用户目录副本是否应让位给系统目录副本（版本不可判读时保守保留用户副本）
#[must_use]
fn should_retire_user_copy(user: Option<u64>, system: Option<u64>) -> bool {
    matches!((user, system), (Some(u), Some(s)) if u < s)
}

/// 若用户目录副本版本落后于系统目录副本，则直接删除该副本让位
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
fn install_extension_to_user_dir(target: &Path, assets_subdir: &str) {
    let Some(assets) = detect_default_assets_path() else {
        return;
    };
    let Some(src_dir) = locate_extension_src_dir(&assets, assets_subdir) else {
        warn!(
            "[桌面集成] 未找到 GNOME 扩展源：系统目录缺失且资源路径 {:?} 下无 {} 目录",
            assets, assets_subdir
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
        if needs_copy && let Err(e) = fs::copy(&src_file, &dst_file) {
            warn!("[桌面集成] 同步 GNOME 扩展文件 {:?} 失败: {}", src_file, e);
        }
    }
}

/// 保障指定 UUID 的 GNOME Shell 扩展文件就绪：系统目录可用时让位给它，否则回退安装到用户目录
pub fn ensure_gnome_extension_files(extension_uuid: &str, assets_subdir: &str) {
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
    install_extension_to_user_dir(&user_dir, assets_subdir);
}

/// 通过 `gnome-extensions` CLI 与 `gsettings` 设置指定扩展的启用或禁用状态
pub fn set_gnome_extension_enabled(extension_uuid: &str, enable: bool) {
    let action = if enable { "enable" } else { "disable" };
    update_gsettings_enabled_extensions(extension_uuid, enable);
    let mut cmd = Command::new("gnome-extensions");
    cmd.args([action, extension_uuid]);
    match run_command_with_timeout(cmd, DEFAULT_COMMAND_TIMEOUT) {
        Ok(out) if out.status.success() => {
            info!(
                "[桌面集成] 已成功{} GNOME 扩展 ({})",
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

/// 同步更新 GNOME `org.gnome.shell enabled-extensions` 列表
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            read_extension_version(&tmp.path().join("absent@x.org")),
            None
        );

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
        let padded = format!(
            "{{\"version\": 7, \"pad\": \"{}\"}}",
            "a".repeat(MAX_EXTENSION_METADATA_BYTES as usize)
        );
        fs::write(dir.join("metadata.json"), padded).expect("写入超大 metadata 失败");
        assert_eq!(read_extension_version(&dir), None);
    }

    #[test]
    fn test_should_retire_user_copy_only_when_strictly_older() {
        assert!(should_retire_user_copy(Some(1), Some(3)));
        assert!(!should_retire_user_copy(Some(3), Some(3)));
        assert!(!should_retire_user_copy(Some(5), Some(3)));
        assert!(!should_retire_user_copy(None, Some(3)));
        assert!(!should_retire_user_copy(Some(1), None));
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
        assert!(system_dir.join("metadata.json").is_file());
    }

    #[test]
    fn test_retire_stale_user_copy_keeps_not_older_or_unknown() {
        let tmp = tempfile::tempdir().expect("创建临时目录失败");
        let newer_user_root = tmp.path().join("newer-user");
        let newer_user = make_extension_dir(&newer_user_root, "x@y.org", "{\"version\": 5}");
        let old_system = make_extension_dir(
            &tmp.path().join("old-system"),
            "x@y.org",
            "{\"version\": 3}",
        );
        retire_stale_user_copy(&newer_user, &old_system);
        assert!(newer_user.exists(), "版本更新的用户副本不得被删除");

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
