//! 系统原生对话框交互与桌面集成
//!
//! # 设计原理
//! - **实现初衷**：封装 Linux 桌面原生 Zenity 文件/目录选择器与 GNOME 输入源扩展联动。
//! - **核心优势**：消除对桌面外部子进程调用的散乱分布，统一错误降级与路径提取契约。
//! - **代价与局限**：依赖宿主系统安装有 `zenity` 或 GNOME 桌面环境组件。

use std::path::PathBuf;

/// 同步更新 GNOME 输入源列表与指示器隐藏扩展
pub fn sync_gnome_indicator_mode(exclusive: bool) {
    if !std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("GNOME"))
        .unwrap_or(false)
    {
        return;
    }

    let sources = "[('ibus', 'waylan'), ('xkb', 'us')]";

    for key in ["sources", "mru-sources"] {
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.desktop.input-sources", key, sources])
            .output();
    }
    let _ = std::process::Command::new("gsettings")
        .args(["set", "org.gnome.desktop.input-sources", "current", "0"])
        .output();
    let _ = std::process::Command::new("ibus")
        .args(["engine", "waylan"])
        .output();

    let action = if exclusive { "enable" } else { "disable" };
    let uuid = "waylan-hide-input-source@waylan.org";
    let _ = std::process::Command::new("gnome-extensions")
        .args([action, uuid])
        .output();
}

/// 调用系统原生对话框 (zenity) 选择外部词库文件
pub fn pick_file_dialog(title: &str, filter_desc: &str, filter_patterns: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("zenity")
        .arg("--file-selection")
        .arg(format!(
            "--file-filter={} | {}",
            filter_desc, filter_patterns
        ))
        .arg("--file-filter=所有文件 (*.*) | *")
        .arg(format!("--title={}", title))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let selected_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_str.is_empty() {
        return None;
    }

    let path = PathBuf::from(&selected_str);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

/// 调用系统原生对话框 (zenity) 选择外部目录（如 ASR 复合模型包目录）
pub fn pick_directory_dialog(title: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("zenity")
        .arg("--file-selection")
        .arg("--directory")
        .arg(format!("--title={}", title))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let selected_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_str.is_empty() {
        return None;
    }

    let path = PathBuf::from(&selected_str);
    if path.is_dir() {
        Some(path)
    } else {
        None
    }
}
