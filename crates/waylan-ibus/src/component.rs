//! IBus 组件描述 XML 生成、用户级组件与图标缓存管理模块
//!
//! # 设计原理
//! - **实现初衷**：在非 root 权限或源码调试环境下，自动将微澜组件注册为用户级 IBus 组件，
//!   并配置 environment.d 使会话启动时能探测到微澜组件。
//! - **核心优势**：自动处理矢量图标缓存与 ~/.local/share/ibus/component/waylan.xml 生成，
//!   无需手动运行复杂安装脚本；复用 `waylan-sys` 平台子进程执行原语，杜绝进程假死与孤儿泄漏。
//! - **代价与局限**：依赖 ibus write-cache 命令与 gtk-update-icon-cache 辅助工具。

use log::{debug, info, warn};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use waylan_core::config::env_var_non_empty;
use waylan_sys::assets::{detect_default_assets_path, resolve_icon_path_or_name};
use waylan_sys::process::{
    run_command_with_timeout, spawn_command_detached, DEFAULT_COMMAND_TIMEOUT,
};

/// 解析微澜 IBus 引擎注册所使用的图标路径（优先绝对路径以确保 GNOME 原生顶栏准确渲染）
pub(crate) fn resolve_engine_icon_path() -> String {
    if let Some(assets) = detect_default_assets_path() {
        resolve_icon_path_or_name(&assets, "waylan")
    } else {
        "waylan".to_string()
    }
}

/// 自动将微澜矢量图标同步注册至用户级 XDG 图标目录 `~/.local/share/icons/hicolor/scalable/apps/`
pub(crate) fn ensure_user_icons_registered(home_path: &Path) {
    let Some(assets_path) = detect_default_assets_path() else {
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
        let mut cmd = Command::new("gtk-update-icon-cache");
        cmd.arg("-f")
            .arg("-t")
            .arg(home_path.join(".local/share/icons/hicolor"));
        spawn_command_detached(cmd, "gtk-update-icon-cache", DEFAULT_COMMAND_TIMEOUT);
    }
}

/// 生成完整的 IBus `<component>` XML 文本
pub fn format_ibus_component_xml(exe_path: &str, version: &str) -> String {
    let icon_path = if exe_path == "/usr/bin/waylan" {
        "waylan".to_string()
    } else {
        resolve_engine_icon_path()
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<component>
    <name>org.freedesktop.IBus.Waylan</name>
    <description>WayLan Chinese Input Method Component</description>
    <exec>{exe} --ibus</exec>
    <version>{ver}</version>
    <author>WayLan Contributors</author>
    <license>Apache-2.0</license>
    <homepage>https://github.com/waylan/waylan</homepage>
    <textdomain>waylan</textdomain>
    <engines>
        <engine>
            <name>waylan</name>
            <longname>微澜 (WayLan)</longname>
            <description>微澜现代纯 Rust 中文拼音输入法</description>
            <language>zh_CN</language>
            <license>Apache-2.0</license>
            <author>WayLan Contributors</author>
            <icon>{icon}</icon>
            <layout>us</layout>
            <rank>99</rank>
            <setup>{exe} settings</setup>
            <version>{ver}</version>
        </engine>
    </engines>
</component>
"#,
        exe = exe_path,
        ver = version,
        icon = icon_path,
    )
}

/// 生成符合 IBus `<engines>` 规范的 XML 文本片段
pub fn format_ibus_engines_xml(exe_path: &str, version: &str) -> String {
    let icon_path = if exe_path == "/usr/bin/waylan" {
        "waylan".to_string()
    } else {
        resolve_engine_icon_path()
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<engines>
    <engine>
        <name>waylan</name>
        <longname>微澜 (WayLan)</longname>
        <description>微澜现代纯 Rust 中文拼音输入法</description>
        <language>zh_CN</language>
        <license>Apache-2.0</license>
        <author>WayLan Contributors</author>
        <icon>{icon}</icon>
        <layout>us</layout>
        <rank>99</rank>
        <setup>{exe} settings</setup>
        <version>{ver}</version>
    </engine>
</engines>
"#,
        exe = exe_path,
        ver = version,
        icon = icon_path,
    )
}

/// 在非打包/用户运行环境下确保微澜 IBus XML 组件描述文件已写入用户目录并更新缓存
///
/// # 设计原理
/// - 遵循"系统级优先"策略：系统级组件描述已存在时不再重复写入用户目录，
///   避免用户级文件覆盖发行版打包的正式组件。
pub fn ensure_ibus_component_installed(exe_path: &str) {
    let Some(home) = env_var_non_empty("HOME") else {
        return;
    };
    let home_path = PathBuf::from(home);

    ensure_user_icons_registered(&home_path);

    let system_component = Path::new("/usr/share/ibus/component/waylan.xml");
    if system_component.is_file() {
        debug!("[微澜 IBus] 系统级 IBus 组件描述已存在: /usr/share/ibus/component/waylan.xml");
        return;
    }

    let user_comp_dir = home_path.join(".local/share/ibus/component");
    if let Err(e) = fs::create_dir_all(&user_comp_dir) {
        warn!("[微澜 IBus 警告] 创建用户组件目录失败: {}", e);
        return;
    }

    let comp_xml_path = user_comp_dir.join("waylan.xml");
    let version = env!("CARGO_PKG_VERSION");
    let xml_content = format_ibus_component_xml(exe_path, version);

    let need_write = match fs::read_to_string(&comp_xml_path) {
        Ok(existing) => existing != xml_content,
        Err(_) => true,
    };

    if need_write && write_user_component_file(&comp_xml_path, &xml_content) {
        refresh_ibus_component_cache();
    }
}

/// 写入用户级 IBus 组件描述文件，返回是否写入成功
fn write_user_component_file(comp_xml_path: &Path, xml_content: &str) -> bool {
    if let Err(e) = fs::write(comp_xml_path, xml_content) {
        warn!("[微澜 IBus 警告] 写入组件描述文件失败: {}", e);
        return false;
    }
    info!(
        "[微澜 IBus] 已成功安装用户级 IBus 组件描述: {:?}",
        comp_xml_path
    );
    true
}

/// 调度 `ibus write-cache` 更新用户级 IBus 注册表缓存
///
/// # 设计原理
/// - 缓存刷新属于尽力而为的收尾动作，失败仅记录调试日志而不影响输入法主体功能。
fn refresh_ibus_component_cache() {
    let mut cmd = Command::new("ibus");
    cmd.arg("write-cache");
    match run_command_with_timeout(cmd, DEFAULT_COMMAND_TIMEOUT) {
        Ok(output) if output.status.success() => {
            info!("[微澜 IBus] 已执行 ibus write-cache 更新用户级 IBus 注册表缓存");
        }
        Ok(output) => {
            debug!(
                "[微澜 IBus] ibus write-cache 执行非零返回: {:?}",
                output.status.code()
            );
        }
        Err(e) => {
            debug!("[微澜 IBus] 调度 ibus write-cache 命令失败或超时: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
