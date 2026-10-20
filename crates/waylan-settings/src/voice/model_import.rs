//! 本地离线语音模型导入事件处理（ASR 复合模型包与 VAD/标点神经网络模型）
//!
//! # 设计原理
//! - **实现初衷**：为设置中心提供用户选择并导入外部本地模型的异步后台任务与 UI 反馈。
//! - **核心优势**：以导入种类枚举收敛目录选择与文件选择两条链路的差异，二者共享同一套
//!   后台复制、结果回传与配置落盘流程；大文件 I/O 派发至独立后台线程，杜绝大模型复制导致 Slint UI 线程挂起假死。
//! - **代价与局限**：模型文件复制需要目标磁盘具有足够的剩余空间。

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;
use waylan_voice::{ModelSubdirectory, VadModelType};

use super::{save_config_and_notify_status, SaveConfigParams};
use crate::callbacks::{pick_directory_dialog_async, pick_file_dialog_async};
use crate::sync::update_config_from_window;
use crate::SettingsWindow;

/// 导入成功后写入配置的回调（采用函数指针以便在多个界面入口间共享同一实现）
type ApplyConfigFn = fn(&mut AppConfig, &str);

/// 本地模型导入种类（决定选择器形态与物理复制方式）
#[derive(Clone, Copy)]
enum ModelImportKind {
    /// ASR 复合模型包（目录选择器 + 目录递归复制）
    AsrPackage,
    /// 单个 ONNX 神经网络模型（文件选择器 + 单文件复制）
    Onnx(ModelSubdirectory),
}

impl ModelImportKind {
    /// 界面提示使用的模型类别名称
    fn label(&self) -> String {
        match self {
            Self::AsrPackage => "ASR".to_string(),
            Self::Onnx(sub) => sub.as_str().to_uppercase(),
        }
    }

    /// 选择对话框标题
    fn dialog_title(&self) -> &'static str {
        match self {
            Self::AsrPackage => "请选择包含 ASR 模型的目录 (需包含 encoder/decoder/tokens)",
            Self::Onnx(ModelSubdirectory::Vad) => "请选择要导入的 VAD 神经网络模型文件 (.onnx)",
            Self::Onnx(ModelSubdirectory::Punct) => "请选择要导入的标点符号预测模型文件 (.onnx)",
            Self::Onnx(ModelSubdirectory::Ns) => "请选择要导入的降噪神经网络模型文件 (.onnx)",
            Self::Onnx(ModelSubdirectory::Asr) => "请选择要导入的 ASR 神经网络模型文件 (.onnx)",
        }
    }

    /// 导入失败时的排查提示
    fn fail_hint(&self) -> String {
        match self {
            Self::AsrPackage => "导入 ASR 复合模型包失败，请检查文件权限与目录完整性".to_string(),
            Self::Onnx(_) => format!("导入 {} 神经网络模型失败，请检查文件权限", self.label()),
        }
    }
}

/// 注册 ASR、VAD、标点与降噪模型导入及目录打开事件
pub(crate) fn register_model_import_callbacks(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
) {
    register_model_import(
        window,
        ModelImportKind::AsrPackage,
        |win, cb| win.on_import_asr_model(cb),
        loaded_config.clone(),
        apply_asr_model_path,
    );
    register_model_import(
        window,
        ModelImportKind::Onnx(ModelSubdirectory::Vad),
        |win, cb| win.on_import_vad_model(cb),
        loaded_config.clone(),
        apply_vad_model_path,
    );
    register_model_import(
        window,
        ModelImportKind::Onnx(ModelSubdirectory::Punct),
        |win, cb| win.on_import_punct_model(cb),
        loaded_config,
        apply_punct_model_path,
    );

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

/// 注册模型导入事件（回调注册方式由调用方指定以适配不同界面入口）
fn register_model_import(
    window: &SettingsWindow,
    kind: ModelImportKind,
    register: impl FnOnce(&SettingsWindow, Box<dyn Fn() + Send + 'static>),
    loaded_config: Arc<Mutex<AppConfig>>,
    apply_fn: ApplyConfigFn,
) {
    let window_weak = window.as_weak();
    register(
        window,
        Box::new(move || {
            if let Some(win) = window_weak.upgrade() {
                handle_model_import(ModelImportOptions {
                    win: &win,
                    current_cfg_cell: &loaded_config,
                    kind,
                    apply_fn,
                });
            }
        }),
    );
}

/// 将导入的 ASR 复合模型包目录写入配置
fn apply_asr_model_path(cfg: &mut AppConfig, path: &str) {
    cfg.voice.offline.model_dir = Some(path.to_string());
}

/// 将导入的 VAD 神经网络模型路径写入配置
fn apply_vad_model_path(cfg: &mut AppConfig, path: &str) {
    cfg.voice.vad.model_path = Some(path.to_string());
    cfg.voice.vad.vad_type = VadModelType::SileroOnnx;
}

/// 将导入的标点符号预测模型路径写入配置
fn apply_punct_model_path(cfg: &mut AppConfig, path: &str) {
    cfg.voice.punct_model_path = Some(path.to_string());
}

/// 模型导入任务选项
struct ModelImportOptions<'a> {
    /// 界面窗口引用
    win: &'a SettingsWindow,
    /// 共享配置单元
    current_cfg_cell: &'a Arc<Mutex<AppConfig>>,
    /// 导入种类
    kind: ModelImportKind,
    /// 导入成功后将目标路径写入配置的回调
    apply_fn: ApplyConfigFn,
}

/// 执行本地模型导入任务（异步弹出选择器并在后台线程完成物理复制）
fn handle_model_import(options: ModelImportOptions<'_>) {
    let ModelImportOptions {
        win,
        current_cfg_cell,
        kind,
        apply_fn,
    } = options;
    let label = kind.label();

    win.set_global_status_is_error(false);
    win.set_global_status(SharedString::from(format!(
        "正在等待选择 {} 模型...",
        label
    )));

    let win_weak = win.as_weak();
    let cfg_cell = current_cfg_cell.clone();

    let on_selected = move |selected: Option<PathBuf>| {
        let _ = slint::invoke_from_event_loop(move || {
            let Some(win) = win_weak.upgrade() else {
                return;
            };
            let Some(src_path) = selected else {
                win.set_global_status(SharedString::from(format!("已取消选择 {} 模型", label)));
                return;
            };
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from(format!(
                "正在后台导入 {} 模型，请稍候...",
                label
            )));

            let win_weak_inner = win.as_weak();
            let cfg_cell_inner = cfg_cell.clone();
            let thread_name = format!("{}-model-importer", kind.label().to_lowercase());
            if let Err(e) = std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    let imported_opt = match kind {
                        ModelImportKind::AsrPackage => {
                            waylan_voice::import_asr_model_from_dir(&src_path)
                        }
                        ModelImportKind::Onnx(sub) => {
                            waylan_voice::import_onnx_model_from_file(sub, &src_path)
                        }
                    };
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = win_weak_inner.upgrade() {
                            handle_imported_model_event(
                                &w,
                                &cfg_cell_inner,
                                kind,
                                imported_opt,
                                apply_fn,
                            );
                        }
                    });
                })
            {
                log::warn!("[偏好设置 警告] 派发模型导入后台线程失败: {}", e);
                win.set_global_status_is_error(true);
                win.set_global_status(SharedString::from(format!("启动模型导入线程失败: {}", e)));
            }
        });
    };

    match kind {
        ModelImportKind::AsrPackage => {
            pick_directory_dialog_async(kind.dialog_title(), on_selected);
        }
        ModelImportKind::Onnx(_) => {
            pick_file_dialog_async(
                kind.dialog_title(),
                "ONNX 神经网络模型 (*.onnx)",
                "*.onnx",
                on_selected,
            );
        }
    }
}

/// 在 Slint 主线程中处理模型导入结果并原子更新配置与界面状态
fn handle_imported_model_event(
    win: &SettingsWindow,
    cfg_cell: &Arc<Mutex<AppConfig>>,
    kind: ModelImportKind,
    imported_opt: Option<PathBuf>,
    apply_fn: ApplyConfigFn,
) {
    let label = kind.label();
    match imported_opt {
        Some(imported_path) => {
            let name = imported_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let mut cfg = cfg_cell.lock().clone();
            update_config_from_window(win, &mut cfg);
            apply_fn(&mut cfg, &imported_path.to_string_lossy());
            save_config_and_notify_status(SaveConfigParams {
                win,
                current_cfg_cell: cfg_cell,
                cfg,
                success_msg: &format!("成功导入 {} 模型: {}", label, name),
                fail_prefix: &format!("保存 {} 模型配置失败", label),
            });
        }
        None => {
            win.set_global_status_is_error(true);
            win.set_global_status(SharedString::from(kind.fail_hint()));
        }
    }
}
