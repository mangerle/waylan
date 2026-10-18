//! 本地离线语音模型导入事件处理（ASR 复合模型包与 VAD/标点神经网络模型）
//!
//! # 设计原理
//! - **实现初衷**：为设置中心提供用户选择并导入外部本地 ONNX 模型文件的异步后台任务与 UI 反馈。
//! - **核心优势**：大文件 I/O 派发至独立后台线程，杜绝大模型复制导致 Slint UI 线程挂起假死。
//! - **代价与局限**：模型文件复制需要目标磁盘具有足够的剩余空间。

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;
use waylan_voice::{import_onnx_model_from_file, ModelSubdirectory};

use super::save_config_and_notify_status;
use crate::callbacks::{pick_directory_dialog, pick_file_dialog};
use crate::sync::update_config_from_window;
use crate::SettingsWindow;

/// 注册 ASR、VAD、标点与降噪模型导入及目录打开事件
pub(crate) fn register_model_import_callbacks(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
) {
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config.clone();
    window.on_import_asr_model(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_import_asr_model(&win, &current_cfg_cell);
        }
    });

    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config.clone();
    window.on_import_vad_model(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_import_onnx_model(
                &win,
                &current_cfg_cell,
                ModelSubdirectory::Vad,
                "请选择要导入的 VAD 神经网络模型文件 (.onnx)",
                |cfg, path| {
                    cfg.voice.vad.model_path = Some(path.to_string());
                    cfg.voice.vad.vad_type = waylan_core::VadModelType::SileroOnnx;
                },
            );
        }
    });

    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
    window.on_import_punct_model(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_import_onnx_model(
                &win,
                &current_cfg_cell,
                ModelSubdirectory::Punct,
                "请选择要导入的标点符号预测模型文件 (.onnx)",
                |cfg, path| {
                    cfg.voice.punct_model_path = Some(path.to_string());
                },
            );
        }
    });

    let window_weak = window.as_weak();
    window.on_import_ns_model(move || {
        if let Some(win) = window_weak.upgrade() {
            win.set_global_status(SharedString::from(
                "系统已内置 RNNoise、DeepFilterNet3 与 GTCRN 三大优质降噪引擎，无需手动导入",
            ));
        }
    });

    window.on_open_models_directory(move || {
        waylan_voice::open_user_models_directory();
    });
}

/// 执行 ASR 复合模型包目录导入（派发后台线程执行大文件递归复制，防止阻塞主 UI 线程）
fn handle_import_asr_model(win: &SettingsWindow, current_cfg_cell: &Arc<Mutex<AppConfig>>) {
    let Some(src_path) =
        pick_directory_dialog("请选择包含 ASR 模型的目录 (需包含 encoder/decoder/tokens)")
    else {
        return;
    };
    win.set_global_status_is_error(false);
    win.set_global_status(SharedString::from("正在后台导入 ASR 复合模型包，请稍候..."));

    let win_weak = win.as_weak();
    let cfg_cell = current_cfg_cell.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("asr-importer".to_string())
        .spawn(move || {
            let imported_opt = waylan_voice::import_asr_model_from_dir(&src_path);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_weak.upgrade() {
                    match imported_opt {
                        Some(imported_path) => {
                            let name = imported_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let mut cfg = cfg_cell.lock().map(|g| g.clone()).unwrap_or_default();
                            update_config_from_window(&w, &mut cfg);
                            cfg.voice.offline.model_dir =
                                Some(imported_path.to_string_lossy().to_string());
                            save_config_and_notify_status(
                                &w,
                                &cfg_cell,
                                cfg,
                                &format!("成功导入 ASR 模型: {}", name),
                                "保存 ASR 模型配置失败",
                            );
                        }
                        None => {
                            w.set_global_status_is_error(true);
                            w.set_global_status(SharedString::from(
                                "导入 ASR 复合模型包失败，请检查文件权限与目录完整性",
                            ));
                        }
                    }
                }
            });
        })
    {
        log::warn!("[偏好设置 警告] 派发 ASR 导入后台线程失败: {}", e);
        win.set_global_status_is_error(true);
        win.set_global_status(SharedString::from(format!("启动模型导入线程失败: {}", e)));
    }
}

/// 执行单个 ONNX 神经网络模型文件导入并应用配置修改（后台线程异步复制）
fn handle_import_onnx_model<F>(
    win: &SettingsWindow,
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
    category: ModelSubdirectory,
    title: &str,
    mut apply_fn: F,
) where
    F: FnMut(&mut AppConfig, &str) + Send + 'static,
{
    let Some(src_path) = pick_file_dialog(title, "ONNX 神经网络模型 (*.onnx)", "*.onnx")
    else {
        return;
    };
    win.set_global_status_is_error(false);
    win.set_global_status(SharedString::from(format!(
        "正在后台导入 {} 神经网络模型，请稍候...",
        category.as_str().to_uppercase()
    )));

    let win_weak = win.as_weak();
    let cfg_cell = current_cfg_cell.clone();
    if let Err(e) = std::thread::Builder::new()
        .name(format!("{}-model-importer", category.as_str()))
        .spawn(move || {
            let imported_opt = import_onnx_model_from_file(category, &src_path);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_weak.upgrade() {
                    match imported_opt {
                        Some(imported_path) => {
                            let name = imported_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let mut cfg = cfg_cell.lock().map(|g| g.clone()).unwrap_or_default();
                            update_config_from_window(&w, &mut cfg);
                            apply_fn(&mut cfg, &imported_path.to_string_lossy());
                            save_config_and_notify_status(
                                &w,
                                &cfg_cell,
                                cfg,
                                &format!(
                                    "成功导入 {} 模型: {}",
                                    category.as_str().to_uppercase(),
                                    name
                                ),
                                &format!("保存 {} 模型配置失败", category.as_str().to_uppercase()),
                            );
                        }
                        None => {
                            w.set_global_status_is_error(true);
                            w.set_global_status(SharedString::from(format!(
                                "导入 {} 神经网络模型失败，请检查文件权限",
                                category.as_str().to_uppercase()
                            )));
                        }
                    }
                }
            });
        })
    {
        log::warn!("[偏好设置 警告] 派发模型导入后台线程失败: {}", e);
        win.set_global_status_is_error(true);
        win.set_global_status(SharedString::from(format!("启动模型导入线程失败: {}", e)));
    }
}
