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

/// 统一执行 `zenity` 对话框子进程（含超时保护、取消/异常退出区分与结果类型校验）
fn run_zenity_picker(args: &[String], expect_dir: bool) -> Option<PathBuf> {
    let mut cmd = Command::new("zenity");
    cmd.args(args);

    let output = match run_command_with_timeout(cmd, DEFAULT_DIALOG_TIMEOUT) {
        Ok(out) => out,
        Err(e) => {
            log::warn!("[偏好设置 警告] 启动或等待 zenity 对话框失败: {e}");
            return None;
        }
    };

    if !output.status.success() {
        // zenity 退出码 1 为用户主动取消，5 为自动超时关闭；其余非 0 状态码记录告警便于排障
        if let Some(code) = output.status.code()
            && code != 1
            && code != 5
        {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::warn!(
                "[偏好设置 警告] zenity 对话框异常退出 (code={}): {}",
                code,
                stderr.trim()
            );
        }
        return None;
    }

    let selected_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_str.is_empty() {
        return None;
    }

    let path = PathBuf::from(&selected_str);
    let valid = if expect_dir {
        path.is_dir()
    } else {
        path.is_file()
    };
    valid.then_some(path)
}

/// 在独立后台线程中异步执行 `zenity` 选择器并通知回调，防止阻塞 Slint 渲染主循环
fn spawn_zenity_picker<F>(
    thread_name: &str,
    picker: impl FnOnce() -> Option<PathBuf> + Send + 'static,
    on_selected: F,
) where
    F: FnOnce(Option<PathBuf>) + Send + 'static,
{
    if let Err(e) = std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || on_selected(picker()))
    {
        log::warn!("[偏好设置 警告] 派发 Zenity 选择器线程失败 ({thread_name}): {e}");
    }
}

/// 调用系统原生对话框 (zenity) 选择外部词库文件（具备超时保护）
pub fn pick_file_dialog(title: &str, filter_desc: &str, filter_patterns: &str) -> Option<PathBuf> {
    let args = vec![
        "--file-selection".to_string(),
        format!("--file-filter={} | {}", filter_desc, filter_patterns),
        "--file-filter=所有文件 (*.*) | *".to_string(),
        format!("--title={}", title),
    ];
    run_zenity_picker(&args, false)
}

/// 调用系统原生对话框 (zenity) 选择外部目录（具备超时保护）
pub fn pick_directory_dialog(title: &str) -> Option<PathBuf> {
    let args = vec![
        "--file-selection".to_string(),
        "--directory".to_string(),
        format!("--title={}", title),
    ];
    run_zenity_picker(&args, true)
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
    spawn_zenity_picker(
        "zenity-file-picker",
        move || pick_file_dialog(title, filter_desc, filter_patterns),
        on_selected,
    );
}

/// 异步调用系统原生对话框 (zenity) 选择外部目录（不阻塞当前 UI 线程）
pub fn pick_directory_dialog_async<F>(title: &'static str, on_selected: F)
where
    F: FnOnce(Option<PathBuf>) + Send + 'static,
{
    spawn_zenity_picker(
        "zenity-dir-picker",
        move || pick_directory_dialog(title),
        on_selected,
    );
}
