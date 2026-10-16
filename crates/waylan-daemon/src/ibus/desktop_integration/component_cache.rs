//! IBus 用户级组件描述文件、图标与环境配置管理
//!
//! # 设计原理
//! - **实现初衷**：在非 root 权限或源码调试环境下，自动将微澜组件注册为用户级 IBus 组件，
//!   并配置 environment.d 使会话启动时能探测到微澜组件。
//! - **核心优势**：自动处理矢量图标缓存与 ~/.local/share/ibus/component/waylan.xml 生成，
//!   无需手动运行复杂安装脚本。
//! - **代价与局限**：依赖 ibus write-cache 命令与 gtk-update-icon-cache 辅助工具。

use log::{debug, info, warn};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::env_var_non_empty;

/// 解析微澜 IBus 引擎注册所使用的图标路径（优先绝对路径以确保 GNOME 原生顶栏准确渲染）
pub(crate) fn resolve_engine_icon_path() -> String {
    if let Some(assets) = crate::tray::item::detect_default_assets_path() {
        crate::tray::item::resolve_icon_path_or_name(&assets, "waylan")
    } else {
        "waylan".to_string()
    }
}

/// 自动将微澜矢量图标同步注册至用户级 XDG 图标目录 `~/.local/share/icons/hicolor/scalable/apps/`
pub(crate) fn ensure_user_icons_registered(home_path: &Path) {
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
            <longname>微澜拼音</longname>
            <description>微澜 (WayLan) - 现代轻量纯 Rust 中文拼音输入法</description>
            <language>zh_CN</language>
            <license>Apache-2.0</license>
            <author>WayLan Contributors</author>
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
        <license>Apache-2.0</license>
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

/// 调用 `ibus write-cache` 将用户级组件编译进 `~/.cache/ibus/bus/registry`
pub(crate) fn rebuild_user_ibus_cache(comp_dir: &Path) {
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
pub(crate) fn ensure_environment_d_config(home_path: &Path, comp_dir: &Path) {
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
        if let Err(e) = fs::write(&conf_path, expected) {
            warn!(
                "[微澜 IBus 警告] 写入 IBus environment.d 配置文件 {:?} 失败: {}",
                conf_path, e
            );
        }
    }
}

/// 安装或更新用户级组件描述与 environment.d 配置
pub(crate) fn install_user_ibus_component(
    exe_path: &str,
    home_path: &Path,
    comp_dir: &Path,
    user_xml: &Path,
) {
    if let Err(e) = fs::create_dir_all(comp_dir) {
        warn!("[微澜 IBus] 创建用户组件目录 {:?} 失败: {}", comp_dir, e);
        return;
    }

    let xml_content = format_ibus_component_xml(exe_path, env!("CARGO_PKG_VERSION"));
    let needs_write = fs::read_to_string(user_xml)
        .map(|old| old != xml_content)
        .unwrap_or(true);

    if needs_write {
        if let Err(e) = fs::write(user_xml, &xml_content) {
            warn!("[微澜 IBus] 写入组件描述文件 {:?} 失败: {}", user_xml, e);
            return;
        }
        info!("[微澜 IBus] 已更新用户级 IBus 组件描述文件: {:?}", user_xml);
        rebuild_user_ibus_cache(comp_dir);
    }

    ensure_environment_d_config(home_path, comp_dir);
}

/// 清理用户级组件描述以避免与系统级 deb 安装包冲突
pub(crate) fn cleanup_redundant_user_component(user_xml: &Path, env_conf: &Path) {
    let mut cleaned = false;
    if user_xml.exists() {
        let _ = fs::remove_file(user_xml);
        cleaned = true;
    }
    if env_conf.exists() {
        let _ = fs::remove_file(env_conf);
        cleaned = true;
    }
    if cleaned {
        info!("[微澜 IBus] 系统级组件已就绪，已清理用户目录冗余描述并刷新缓存。");
        let _ = Command::new("ibus").arg("write-cache").output();
    }
}

/// 确保微澜 IBus 组件描述与图标正确就绪
pub(crate) fn ensure_ibus_component_installed(exe_path: &str) {
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
        cleanup_redundant_user_component(&user_xml, &env_conf);
    } else {
        install_user_ibus_component(exe_path, &home_path, &comp_dir, &user_xml);
    }
}
