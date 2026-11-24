//! 离线语音大模型一键下载与即时启用事件处理模块
//!
//! # 设计原理
//! - **实现初衷**：为设置中心提供在线模型一键异步高速下载（ModelScope）与零卡死 UI 进度条反馈，
//!   并在下载完成后支持一键落盘与无感热重载生效。
//! - **核心优势**：
//!   1. 下载与解压全程派发至独立的后台系统线程，彻底消除大文件下载导致 Slint 界面卡顿或假死；
//!   2. 采用 `upgrade_in_event_loop` 将平滑节流的下载进度抛回 Slint 渲染循环，保证 60fps 丝滑交互；
//!   3. 启用模型时直接持久化配置至 `~/.config/waylan/config.toml`，后台语音守护线程通过热重载自动换入。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use log::{error, info};
use parking_lot::Mutex;
use slint::{ComponentHandle, Model, SharedString};
use waylan_core::config::AppConfig;
use waylan_voice::{download_and_extract_model, find_preset_model_by_id};

use super::{apply_asr_model_selection, commit_config_transaction};
use crate::SettingsWindow;
use crate::sync::sync_asr_models_to_window;

/// 注册预置离线大模型的一键下载与启用回调
pub(crate) fn register_model_download_callbacks(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
) {
    let is_downloading = Arc::new(AtomicBool::new(false));
    register_download_action(window, loaded_config.clone(), is_downloading);
    register_activate_action(window, loaded_config);
}

/// 注册点击【下载并部署】事件
fn register_download_action(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
    is_downloading: Arc<AtomicBool>,
) {
    let window_weak = window.as_weak();

    window.on_download_asr_model(move |model_id_slint| {
        let model_id = model_id_slint.to_string();
        let Some(meta) = find_preset_model_by_id(&model_id) else {
            if let Some(win) = window_weak.upgrade() {
                win.set_global_status_is_error(true);
                win.set_global_status(SharedString::from(format!(
                    "未知的语音模型标识: {}",
                    model_id
                )));
            }
            return;
        };

        if is_downloading
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            if let Some(win) = window_weak.upgrade() {
                win.set_global_status_is_error(false);
                win.set_global_status(SharedString::from(
                    "已有模型正在下载部署中，请等待当前任务完成",
                ));
            }
            return;
        }

        let model_name = meta.name;
        info!(
            "[模型下载] 用户触发离线模型一键高速下载: {} ({})",
            model_name, model_id
        );

        if let Some(win) = window_weak.upgrade() {
            set_model_downloading_ui(&win, &model_id, 0.0, "正在连接 ModelScope 高速源...");
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from(format!(
                "正在后台高速下载模型 [{}]，请稍候...",
                model_name
            )));
        }

        let win_weak_clone = window_weak.clone();
        let cfg_clone = loaded_config.clone();
        let downloading_guard = Arc::clone(&is_downloading);
        thread::spawn(move || {
            run_download_worker(
                win_weak_clone,
                cfg_clone,
                model_id,
                model_name,
                downloading_guard,
            );
        });
    });
}

/// 后台执行模型下载并在完成后通过主事件循环更新界面状态
fn run_download_worker(
    win_weak: slint::Weak<SettingsWindow>,
    cfg_cell: Arc<Mutex<AppConfig>>,
    model_id: String,
    model_name: &'static str,
    is_downloading: Arc<AtomicBool>,
) {
    let win_for_progress = win_weak.clone();
    let id_for_progress = model_id.clone();

    let result = download_and_extract_model(&model_id, move |progress, speed_desc| {
        let id_sub = id_for_progress.clone();
        let desc_sub = speed_desc.to_string();
        let _ = win_for_progress.upgrade_in_event_loop(move |win| {
            set_model_downloading_ui(&win, &id_sub, progress, &desc_sub);
        });
    });

    is_downloading.store(false, Ordering::Release);

    let _ = win_weak.upgrade_in_event_loop(move |win| {
        let current_cfg = cfg_cell.lock().clone();
        sync_asr_models_to_window(&win, &current_cfg);
        apply_download_result_ui(&win, &model_id, model_name, result);
    });
}

/// 将下载或解压结果反馈至界面状态栏与对应模型卡片
fn apply_download_result_ui(
    win: &SettingsWindow,
    model_id: &str,
    model_name: &str,
    result: Result<std::path::PathBuf, waylan_voice::VoiceError>,
) {
    match result {
        Ok(dest_dir) => {
            info!(
                "[模型下载] 模型 [{}] 下载解压成功: {}",
                model_name,
                dest_dir.display()
            );
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from(format!(
                "模型 [{}] 成功部署至本地！点击【立即启用】即可使用",
                model_name
            )));
        }
        Err(e) => {
            error!("[模型下载] 模型 [{}] 下载失败: {}", model_name, e);
            win.set_global_status_is_error(true);
            let clean_err = format!("{}", e)
                .replace("语音模型管理或下载部署失败，原因: ", "")
                .replace("语音模型管理或下载部署失败: ", "");
            win.set_global_status(SharedString::from(format!(
                "下载 [{}] 失败: {}",
                model_name, clean_err
            )));

            let asr_models = win.get_asr_models();
            for i in 0..asr_models.row_count() {
                if let Some(mut item) = asr_models.row_data(i)
                    && item.id.as_str() == model_id
                {
                    item.state = 0;
                    item.progress = 0.0;
                    item.progress_text = SharedString::from(clean_err);
                    asr_models.set_row_data(i, item);
                    break;
                }
            }
        }
    }
}

/// 注册点击【立即启用】/【设为当前生效】事件
fn register_activate_action(window: &SettingsWindow, loaded_config: Arc<Mutex<AppConfig>>) {
    let window_weak = window.as_weak();

    window.on_activate_asr_model(move |model_id_slint| {
        let model_id = model_id_slint.to_string();
        let Some(win) = window_weak.upgrade() else {
            return;
        };

        let _ = commit_config_transaction(
            &win,
            &loaded_config,
            "保存语音模型配置失败",
            true,
            |cfg| apply_asr_model_selection(cfg, &model_id, true),
        );
    });
}

/// 原地更新 UI 模型卡片列表中指定项为下载中状态与进度（避免每次重建整个 VecModel）
fn set_model_downloading_ui(
    win: &SettingsWindow,
    target_id: &str,
    progress: f32,
    progress_text: &str,
) {
    let models_rc = win.get_asr_models();
    for idx in 0..models_rc.row_count() {
        if let Some(mut item) = models_rc.row_data(idx)
            && item.id.as_str() == target_id
        {
            item.state = 1;
            item.progress = progress;
            item.progress_text = SharedString::from(progress_text);
            models_rc.set_row_data(idx, item);
            break;
        }
    }
}
