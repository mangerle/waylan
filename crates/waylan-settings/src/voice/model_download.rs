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
use std::thread;

use log::{error, info};
use parking_lot::Mutex;
use slint::{ComponentHandle, Model, SharedString};
use waylan_core::config::AppConfig;
use waylan_voice::{
    download_and_extract_model, find_preset_model_by_id, get_preset_model_dest_dir,
    is_preset_model_installed, VoiceEngineType,
};

use super::{save_config_and_notify_status, SaveConfigParams};
use crate::sync::sync_asr_models_to_window;
use crate::SettingsWindow;

/// 注册预置离线大模型的一键下载与启用回调
pub(crate) fn register_model_download_callbacks(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
) {
    register_download_action(window, loaded_config.clone());
    register_activate_action(window, loaded_config);
}

/// 注册点击【下载并部署】事件
fn register_download_action(window: &SettingsWindow, loaded_config: Arc<Mutex<AppConfig>>) {
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

        let model_name = meta.name;
        info!(
            "[模型下载] 用户触发离线模型一键高速下载: {} ({})",
            model_name, model_id
        );

        // 1. 立即在 UI 上将该模型置为下载中状态
        if let Some(win) = window_weak.upgrade() {
            set_model_downloading_ui(&win, &model_id, 0.0, "正在连接 ModelScope 高速源...");
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from(format!(
                "正在后台高速下载模型 [{}]，请稍候...",
                model_name
            )));
        }

        // 2. 派发至独立后台线程执行网络流式下载与解压
        let win_weak_clone = window_weak.clone();
        let cfg_clone = loaded_config.clone();
        let id_for_thread = model_id.clone();

        thread::spawn(move || {
            let win_for_progress = win_weak_clone.clone();
            let id_for_progress = id_for_thread.clone();

            let result = download_and_extract_model(&id_for_thread, move |progress, speed_desc| {
                let id_sub = id_for_progress.clone();
                let desc_sub = speed_desc.to_string();
                let _ = win_for_progress.upgrade_in_event_loop(move |win| {
                    set_model_downloading_ui(&win, &id_sub, progress, &desc_sub);
                });
            });

            // 3. 下载与解压完成后的主线程回传处理
            let _ = win_weak_clone.upgrade_in_event_loop(move |win| {
                let current_cfg = cfg_clone.lock().clone();
                sync_asr_models_to_window(&win, &current_cfg);

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

                        // 精简并提取核心错误描述，消除冗余包裹前缀
                        let err_str = format!("{}", e);
                        let clean_err = err_str
                            .replace("语音模型管理或下载部署失败，原因: ", "")
                            .replace("语音模型管理或下载部署失败: ", "");

                        win.set_global_status(SharedString::from(format!(
                            "下载 [{}] 失败: {}",
                            model_name, clean_err
                        )));

                        // 在卡片上显式记录并呈现失败原因，防止底部状态栏空间受限导致截断
                        let asr_models = win.get_asr_models();
                        let count = asr_models.row_count();
                        for i in 0..count {
                            if let Some(mut item) = asr_models.row_data(i) {
                                if item.id.as_str() == id_for_thread.as_str() {
                                    item.state = 0; // 回到未下载
                                    item.progress = 0.0;
                                    item.progress_text = SharedString::from(clean_err.clone());
                                    asr_models.set_row_data(i, item);
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        });
    });
}

/// 注册点击【立即启用】/【设为当前生效】事件
fn register_activate_action(window: &SettingsWindow, loaded_config: Arc<Mutex<AppConfig>>) {
    let window_weak = window.as_weak();

    window.on_activate_asr_model(move |model_id_slint| {
        let model_id = model_id_slint.to_string();
        let Some(win) = window_weak.upgrade() else {
            return;
        };

        if let Some(meta) = find_preset_model_by_id(&model_id) {
            if !is_preset_model_installed(meta) {
                win.set_global_status_is_error(true);
                win.set_global_status(SharedString::from(format!(
                    "模型 [{}] 尚未在本地下载，请先点击【下载并部署】",
                    meta.name
                )));
                return;
            }

            let Some(dest_dir) = get_preset_model_dest_dir(meta) else {
                return;
            };

            let mut new_cfg = loaded_config.lock().clone();
            new_cfg.voice.engine_type = VoiceEngineType::Offline;
            new_cfg.voice.selected_asr = None;
            new_cfg.voice.offline.model_type = meta.model_type;
            new_cfg.voice.offline.model_dir = Some(dest_dir.to_string_lossy().to_string());

            let model_name = meta.name;
            save_config_and_notify_status(SaveConfigParams {
                win: &win,
                current_cfg_cell: &loaded_config,
                cfg: new_cfg,
                success_msg: &format!(
                    "已成功启用 [{}] 离线语音识别引擎，配置已落盘热重载生效",
                    model_name
                ),
                fail_prefix: "保存语音模型配置失败",
            });
        } else if let Some(local_path) = model_id.strip_prefix("local:") {
            let mut new_cfg = loaded_config.lock().clone();
            new_cfg.voice.engine_type = VoiceEngineType::Offline;
            new_cfg.voice.selected_asr = None;
            new_cfg.voice.offline.model_dir = Some(local_path.to_string());

            save_config_and_notify_status(SaveConfigParams {
                win: &win,
                current_cfg_cell: &loaded_config,
                cfg: new_cfg,
                success_msg: "已成功切换至本地自定义 ASR 模型，配置已落盘热重载生效",
                fail_prefix: "保存语音模型配置失败",
            });
        } else if let Some(cloud_id) = model_id.strip_prefix("cloud:") {
            let mut new_cfg = loaded_config.lock().clone();
            let profile = new_cfg
                .voice
                .cloud_profiles
                .iter()
                .find(|p| p.id == cloud_id)
                .cloned();

            if let Some(p) = profile {
                new_cfg.voice.engine_type = VoiceEngineType::Cloud;
                new_cfg.voice.selected_asr = Some(format!("cloud:{}", cloud_id));
                new_cfg.voice.cloud.endpoint = p.endpoint.clone();
                new_cfg.voice.cloud.api_key = p.api_key.clone();
                new_cfg.voice.cloud.model = p.model.clone();

                save_config_and_notify_status(SaveConfigParams {
                    win: &win,
                    current_cfg_cell: &loaded_config,
                    cfg: new_cfg,
                    success_msg: &format!("已成功切换至 [{}] 云端转写模式，配置已落盘生效", p.name),
                    fail_prefix: "保存云端配置失败",
                });
            }
        }
    });
}

/// 更新 UI 模型卡片列表中指定项为下载中状态与进度
fn set_model_downloading_ui(
    win: &SettingsWindow,
    target_id: &str,
    progress: f32,
    progress_text: &str,
) {
    let models_rc = win.get_asr_models();
    let row_count = models_rc.row_count();
    let mut updated = Vec::with_capacity(row_count);

    for idx in 0..row_count {
        if let Some(mut item) = models_rc.row_data(idx) {
            if item.id.as_str() == target_id {
                item.state = 1; // 下载中
                item.progress = progress;
                item.progress_text = SharedString::from(progress_text);
            }
            updated.push(item);
        }
    }

    win.set_asr_models(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(updated),
    )));
}
