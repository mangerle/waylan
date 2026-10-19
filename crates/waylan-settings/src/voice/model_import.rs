//! 本地离线语音模型导入事件处理（ASR 复合模型包与 VAD/标点神经网络模型）
//!
//! # 设计原理
//! - **实现初衷**：为设置中心提供用户选择并导入外部本地 ONNX 模型文件的异步后台任务与 UI 反馈。
//! - **核心优势**：大文件 I/O 派发至独立后台线程，杜绝大模型复制导致 Slint UI 线程挂起假死。
//! - **代价与局限**：模型文件复制需要目标磁盘具有足够的剩余空间。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;
use waylan_voice::{import_onnx_model_from_file, ModelSubdirectory, VadModelType};

use super::{save_config_and_notify_status, SaveConfigParams};
use crate::callbacks::{pick_directory_dialog_async, pick_file_dialog_async};
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
            handle_import_onnx_model(OnnxModelImportOptions {
                win: &win,
                current_cfg_cell: &current_cfg_cell,
                category: ModelSubdirectory::Vad,
                title: "请选择要导入的 VAD 神经网络模型文件 (.onnx)",
                apply_fn: |cfg: &mut AppConfig, path: &str| {
                    cfg.voice.vad.model_path = Some(path.to_string());
                    cfg.voice.vad.vad_type = VadModelType::SileroOnnx;
                },
            });
        }
    });

    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
    window.on_import_punct_model(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_import_onnx_model(OnnxModelImportOptions {
                win: &win,
                current_cfg_cell: &current_cfg_cell,
                category: ModelSubdirectory::Punct,
                title: "请选择要导入的标点符号预测模型文件 (.onnx)",
                apply_fn: |cfg: &mut AppConfig, path: &str| {
                    cfg.voice.punct_model_path = Some(path.to_string());
                },
            });
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

/// 执行 ASR 复合模型包目录导入（对话框异步弹出并在后台线程执行大文件递归复制，防止阻塞主 UI 线程）
fn handle_import_asr_model(win: &SettingsWindow, current_cfg_cell: &Arc<Mutex<AppConfig>>) {
    win.set_global_status_is_error(false);
    win.set_global_status(SharedString::from("正在等待选择 ASR 模型目录..."));

    let win_weak = win.as_weak();
    let cfg_cell = current_cfg_cell.clone();

    pick_directory_dialog_async(
        "请选择包含 ASR 模型的目录 (需包含 encoder/decoder/tokens)",
        move |selected| {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(win) = win_weak.upgrade() else {
                    return;
                };
                let Some(src_path) = selected else {
                    win.set_global_status(SharedString::from("已取消选择 ASR 模型目录"));
                    return;
                };
                win.set_global_status_is_error(false);
                win.set_global_status(SharedString::from("正在后台导入 ASR 复合模型包，请稍候..."));

                let win_weak_inner = win.as_weak();
                let cfg_cell_inner = cfg_cell.clone();
                if let Err(e) = std::thread::Builder::new()
                    .name("asr-importer".to_string())
                    .spawn(move || {
                        let imported_opt = waylan_voice::import_asr_model_from_dir(&src_path);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = win_weak_inner.upgrade() {
                                match imported_opt {
                                    Some(imported_path) => {
                                        let name = imported_path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .to_string();
                                        let mut cfg = cfg_cell_inner
                                            .lock()
                                            .map(|g| g.clone())
                                            .unwrap_or_default();
                                        update_config_from_window(&w, &mut cfg);
                                        cfg.voice.offline.model_dir =
                                            Some(imported_path.to_string_lossy().to_string());
                                        save_config_and_notify_status(SaveConfigParams {
                                            win: &w,
                                            current_cfg_cell: &cfg_cell_inner,
                                            cfg,
                                            success_msg: &format!("成功导入 ASR 模型: {}", name),
                                            fail_prefix: "保存 ASR 模型配置失败",
                                        });
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
                    win.set_global_status(SharedString::from(format!(
                        "启动模型导入线程失败: {}",
                        e
                    )));
                }
            });
        },
    );
}

/// 单个 ONNX 神经网络模型文件导入选项
struct OnnxModelImportOptions<'a, F> {
    win: &'a SettingsWindow,
    current_cfg_cell: &'a Arc<Mutex<AppConfig>>,
    category: ModelSubdirectory,
    title: &'static str,
    apply_fn: F,
}

/// 模型导入后台任务上下文（保存前台弱引用与共享配置单元）
struct ModelImportContext {
    win_weak: slint::Weak<SettingsWindow>,
    cfg_cell: Arc<Mutex<AppConfig>>,
    category: ModelSubdirectory,
}

/// 导入模型配置应用上下文
struct ModelApplyContext<'a, F> {
    win: &'a SettingsWindow,
    cfg_cell: &'a Arc<Mutex<AppConfig>>,
    category: ModelSubdirectory,
    apply_fn: F,
}

/// 执行单个 ONNX 神经网络模型文件导入并应用配置修改（对话框异步弹出并在后台线程异步复制）
fn handle_import_onnx_model<F>(options: OnnxModelImportOptions<'_, F>)
where
    F: FnMut(&mut AppConfig, &str) + Send + 'static,
{
    let OnnxModelImportOptions {
        win,
        current_cfg_cell,
        category,
        title,
        apply_fn,
    } = options;

    win.set_global_status_is_error(false);
    win.set_global_status(SharedString::from("正在等待选择模型文件..."));

    let win_weak = win.as_weak();
    let cfg_cell = current_cfg_cell.clone();

    pick_file_dialog_async(
        title,
        "ONNX 神经网络模型 (*.onnx)",
        "*.onnx",
        move |selected| {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(win) = win_weak.upgrade() else {
                    return;
                };
                let Some(src_path) = selected else {
                    win.set_global_status(SharedString::from("已取消选择模型文件"));
                    return;
                };
                win.set_global_status_is_error(false);
                win.set_global_status(SharedString::from(format!(
                    "正在后台导入 {} 神经网络模型，请稍候...",
                    category.as_str().to_uppercase()
                )));

                let win_weak_inner = win.as_weak();
                let cfg_cell_inner = cfg_cell.clone();
                if let Err(e) = std::thread::Builder::new()
                    .name(format!("{}-model-importer", category.as_str()))
                    .spawn(move || {
                        run_model_import_thread(
                            ModelImportContext {
                                win_weak: win_weak_inner,
                                cfg_cell: cfg_cell_inner,
                                category,
                            },
                            src_path,
                            apply_fn,
                        );
                    })
                {
                    log::warn!("[偏好设置 警告] 派发模型导入后台线程失败: {}", e);
                    win.set_global_status_is_error(true);
                    win.set_global_status(SharedString::from(format!(
                        "启动模型导入线程失败: {}",
                        e
                    )));
                }
            });
        },
    );
}

/// 成功导入模型后原子更新配置与主界面状态
fn apply_imported_model_to_config_and_ui<F>(
    mut ctx: ModelApplyContext<'_, F>,
    imported_path: PathBuf,
) where
    F: FnMut(&mut AppConfig, &str),
{
    let name = imported_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut cfg = ctx.cfg_cell.lock().map(|g| g.clone()).unwrap_or_default();
    update_config_from_window(ctx.win, &mut cfg);
    (ctx.apply_fn)(&mut cfg, &imported_path.to_string_lossy());
    save_config_and_notify_status(SaveConfigParams {
        win: ctx.win,
        current_cfg_cell: ctx.cfg_cell,
        cfg,
        success_msg: &format!(
            "成功导入 {} 模型: {}",
            ctx.category.as_str().to_uppercase(),
            name
        ),
        fail_prefix: &format!("保存 {} 模型配置失败", ctx.category.as_str().to_uppercase()),
    });
}

/// 在 Slint 主线程中处理模型导入结果事件
fn handle_imported_model_event<F>(ctx: ModelApplyContext<'_, F>, imported_opt: Option<PathBuf>)
where
    F: FnMut(&mut AppConfig, &str),
{
    match imported_opt {
        Some(imported_path) => {
            apply_imported_model_to_config_and_ui(ctx, imported_path);
        }
        None => {
            ctx.win.set_global_status_is_error(true);
            ctx.win.set_global_status(SharedString::from(format!(
                "导入 {} 神经网络模型失败，请检查文件权限",
                ctx.category.as_str().to_uppercase()
            )));
        }
    }
}

/// 后台线程执行具体的模型文件复制并通知前台
fn run_model_import_thread<F>(ctx: ModelImportContext, src_path: PathBuf, apply_fn: F)
where
    F: FnMut(&mut AppConfig, &str) + Send + 'static,
{
    let imported_opt = import_onnx_model_from_file(ctx.category, &src_path);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ctx.win_weak.upgrade() {
            handle_imported_model_event(
                ModelApplyContext {
                    win: &w,
                    cfg_cell: &ctx.cfg_cell,
                    category: ctx.category,
                    apply_fn,
                },
                imported_opt,
            );
        }
    });
}
