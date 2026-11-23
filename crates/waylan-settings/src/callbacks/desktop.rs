//! 系统原生对话框交互与桌面集成
//!
//! # 设计原理
//! - **实现初衷**：封装 Linux 桌面原生 Zenity 文件/目录选择器与 GNOME 输入源扩展联动。
//! - **核心优势**：消除对桌面外部子进程调用的散乱分布，统一错误降级与路径提取契约。
//! - **代价与局限**：依赖宿主系统安装有 `zenity` 或 GNOME 桌面环境组件。

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use waylan_core::config::AppConfig;
use waylan_sys::desktop::{
    GnomeExtensionIntent, WAYLAN_GNOME_EXTENSION_UUID, WAYLAN_IBUS_ENGINE_NAME,
    WAYLAN_IBUS_SOURCE_ENTRY, activate_ibus_engine, enqueue_desktop_task,
    ensure_gnome_extension_files, ensure_input_source_prioritized, is_gnome_desktop,
    set_gnome_extension_enabled,
};
use waylan_sys::process::run_command_with_timeout;

/// 默认系统原生对话框等待超时时间（2分钟）
const DEFAULT_DIALOG_TIMEOUT: Duration = Duration::from_secs(120);

/// 由配置解析 GNOME 扩展启用意图
///
/// 扩展的"隐藏顶栏指示器"与"渲染微澜候选框"两职分别由两个独立设置项决定，
/// 加载条件取并集，故必须同时读取；判定谓词统一由 `AppConfig` 提供。
#[must_use]
pub fn gnome_extension_intent(cfg: &AppConfig) -> GnomeExtensionIntent {
    GnomeExtensionIntent {
        hide_indicator: cfg.desktop.exclusive_input_source,
        custom_candidate_view: cfg.uses_custom_candidate_view(),
    }
}

/// 同步更新 GNOME 输入源列表与配套扩展加载状态（通过串行后台任务队列安全执行）
pub fn sync_gnome_indicator_mode(intent: GnomeExtensionIntent) {
    if !is_gnome_desktop() {
        return;
    }
    enqueue_desktop_task(move || {
        ensure_input_source_prioritized(
            WAYLAN_IBUS_SOURCE_ENTRY,
            "('xkb', 'us')",
            intent.hide_indicator,
        );
        ensure_gnome_extension_files(WAYLAN_GNOME_EXTENSION_UUID, "gnome-extension");
        set_gnome_extension_enabled(WAYLAN_GNOME_EXTENSION_UUID, intent.should_enable());
        activate_ibus_engine(WAYLAN_IBUS_ENGINE_NAME);
    });
}

/// 调用系统原生对话框 (zenity) 选择外部词库文件（具备超时保护）
pub fn pick_file_dialog(title: &str, filter_desc: &str, filter_patterns: &str) -> Option<PathBuf> {
    let mut cmd = Command::new("zenity");
    cmd.arg("--file-selection")
        .arg(format!(
            "--file-filter={} | {}",
            filter_desc, filter_patterns
        ))
        .arg("--file-filter=所有文件 (*.*) | *")
        .arg(format!("--title={}", title));

    let output = run_command_with_timeout(cmd, DEFAULT_DIALOG_TIMEOUT).ok()?;

    if !output.status.success() {
        return None;
    }

    let selected_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_str.is_empty() {
        return None;
    }

    let path = PathBuf::from(&selected_str);
    if path.is_file() { Some(path) } else { None }
}

/// 调用系统原生对话框 (zenity) 选择外部目录（具备超时保护）
pub fn pick_directory_dialog(title: &str) -> Option<PathBuf> {
    let mut cmd = Command::new("zenity");
    cmd.arg("--file-selection")
        .arg("--directory")
        .arg(format!("--title={}", title));

    let output = run_command_with_timeout(cmd, DEFAULT_DIALOG_TIMEOUT).ok()?;

    if !output.status.success() {
        return None;
    }

    let selected_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_str.is_empty() {
        return None;
    }

    let path = PathBuf::from(&selected_str);
    if path.is_dir() { Some(path) } else { None }
}

/// 异步调用系统原生对话框 (zenity) 选择外部文件，并通过回调接收路径结果（不阻塞当前 UI 线程）
pub fn pick_file_dialog_async<F>(
    title: &'static str,
    filter_desc: &'static str,
    filter_patterns: &'static str,
    on_selected: F,
) where
    F: FnOnce(Option<PathBuf>) + Send + 'static,
{
    let spawn_res = std::thread::Builder::new()
        .name("zenity-file-picker".to_string())
        .spawn(move || {
            let path = pick_file_dialog(title, filter_desc, filter_patterns);
            on_selected(path);
        });

    if let Err(e) = spawn_res {
        log::warn!("[偏好设置 警告] 派发 Zenity 文件选择线程失败: {}", e);
    }
}

/// 异步调用系统原生对话框 (zenity) 选择外部目录（不阻塞当前 UI 线程）
pub fn pick_directory_dialog_async<F>(title: &'static str, on_selected: F)
where
    F: FnOnce(Option<PathBuf>) + Send + 'static,
{
    let spawn_res = std::thread::Builder::new()
        .name("zenity-dir-picker".to_string())
        .spawn(move || {
            let path = pick_directory_dialog(title);
            on_selected(path);
        });

    if let Err(e) = spawn_res {
        log::warn!("[偏好设置 警告] 派发 Zenity 目录选择线程失败: {}", e);
    }
}
