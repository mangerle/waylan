//! 本地离线语音模型导入事件处理（ASR 复合模型包与 VAD 神经网络模型）
//!
//! # 设计原理
//! - **实现初衷**：为设置中心提供用户选择并导入外部本地模型的异步后台任务与 UI 反馈。
//! - **核心优势**：以导入种类枚举收敛目录选择与文件选择两条链路的差异，二者共享同一套
//!   后台复制、结果回传与配置落盘流程；大文件 I/O 派发至独立后台线程，杜绝大模型复制导致 Slint UI 线程挂起假死。
//! - **代价与局限**：模型文件复制需要目标磁盘具有足够的剩余空间。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;
use waylan_voice::{ModelSubdirectory, VadModelType, VoiceEngineType, infer_offline_model_type};

use super::commit_config_transaction;
use crate::SettingsWindow;
use crate::callbacks::{TaskRunningGuard, pick_directory_dialog_async, pick_file_dialog_async};
use crate::sync::update_config_from_window;

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

/// 注册 ASR、VAD 与降噪模型导入及目录打开事件
pub(crate) fn register_model_import_callbacks(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
) {
    let is_importing = Arc::new(AtomicBool::new(false));
    register_model_import(
        ModelImportRegistration {
            window,
            kind: ModelImportKind::AsrPackage,
            loaded_config: loaded_config.clone(),
            is_importing: is_importing.clone(),
            apply_fn: apply_asr_model_path,
        },
        |win, cb| win.on_import_asr_model(cb),
    );
    register_model_import(
        ModelImportRegistration {
            window,
            kind: ModelImportKind::Onnx(ModelSubdirectory::Vad),
            loaded_config,
            is_importing,
            apply_fn: apply_vad_model_path,
        },
        |win, cb| win.on_import_vad_model(cb),
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

/// 模型导入回调注册参数
///
/// # 设计原理
/// - **实现初衷**：每种模型导入都需要"窗口句柄、导入种类、共享配置、落盘回调"四要素，
///   与注册方式闭包混在平铺参数列表中，易在多个注册点发生落盘回调错配。
/// - **核心优势**：以参数对象聚合四要素，调用点按名传参，注册闭包单独保持在末位便于阅读。
/// - **代价与局限**：多引入一个结构体类型。
struct ModelImportRegistration<'a> {
    /// 设置中心窗口句柄
    window: &'a SettingsWindow,
    /// 本次导入的模型种类
    kind: ModelImportKind,
    /// 与界面共享的持久化配置
    loaded_config: Arc<Mutex<AppConfig>>,
    /// 模型导入防重入标志
    is_importing: Arc<AtomicBool>,
    /// 导入成功后写入配置的回调
    apply_fn: ApplyConfigFn,
}

/// 注册模型导入事件（回调注册方式由调用方指定以适配不同界面入口）
fn register_model_import(
    params: ModelImportRegistration<'_>,
    register: impl FnOnce(&SettingsWindow, Box<dyn Fn() + Send + 'static>),
) {
    let ModelImportRegistration {
        window,
        kind,
        loaded_config,
        is_importing,
        apply_fn,
    } = params;
    let window_weak = window.as_weak();
    register(
        window,
        Box::new(move || {
            if let Some(win) = window_weak.upgrade() {
                handle_model_import(ModelImportOptions {
                    win: &win,
                    current_cfg_cell: &loaded_config,
                    is_importing: &is_importing,
                    kind,
                    apply_fn,
                });
            }
        }),
    );
}

/// 将导入的 ASR 复合模型包目录写入配置，并同步切换为离线引擎与推断的模型架构
fn apply_asr_model_path(cfg: &mut AppConfig, path: &str) {
    cfg.voice.engine_type = VoiceEngineType::Offline;
    cfg.voice.selected_asr = None;
    cfg.voice.offline.model_dir = Some(path.to_string());
    cfg.voice.offline.model_type = infer_offline_model_type(Path::new(path));
}

/// 将导入的 VAD 神经网络模型路径写入配置
fn apply_vad_model_path(cfg: &mut AppConfig, path: &str) {
    cfg.voice.vad.model_path = Some(path.to_string());
    cfg.voice.vad.vad_type = VadModelType::SileroOnnx;
}

/// 模型导入任务选项
struct ModelImportOptions<'a> {
    /// 界面窗口引用
    win: &'a SettingsWindow,
    /// 共享配置单元
    current_cfg_cell: &'a Arc<Mutex<AppConfig>>,
    /// 导入防重入标志
    is_importing: &'a Arc<AtomicBool>,
    /// 导入种类
    kind: ModelImportKind,
    /// 导入成功后将目标路径写入配置的回调
    apply_fn: ApplyConfigFn,
}

/// 待派发的模型导入上下文
#[derive(Clone)]
struct PendingModelImport {
    /// 界面窗口弱引用（升级失败表示窗口已销毁，流程应静默终止）
    win_weak: slint::Weak<SettingsWindow>,
    /// 共享配置单元
    cfg_cell: Arc<Mutex<AppConfig>>,
    /// 导入防重入标志
    is_importing: Arc<AtomicBool>,
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
        is_importing,
        kind,
        apply_fn,
    } = options;
    if is_importing
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        win.set_global_status_is_error(false);
        win.set_global_status(SharedString::from(
            "已有本地模型导入任务正在进行中，请稍候...",
        ));
        return;
    }

    let label = kind.label();
    win.set_global_status_is_error(false);
    win.set_global_status(SharedString::from(format!(
        "正在等待选择 {} 模型...",
        label
    )));

    let pending = PendingModelImport {
        win_weak: win.as_weak(),
        cfg_cell: current_cfg_cell.clone(),
        is_importing: is_importing.clone(),
        kind,
        apply_fn,
    };
    let on_selected = move |selected| queue_model_import_dispatch(pending, selected);

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

/// 接收文件选择结果并回投 Slint 主线程派发模型导入后台任务
fn queue_model_import_dispatch(pending: PendingModelImport, selected: Option<PathBuf>) {
    let flag_fallback = pending.is_importing.clone();
    if slint::invoke_from_event_loop(move || {
        dispatch_model_import(pending, selected);
    })
    .is_err()
    {
        flag_fallback.store(false, Ordering::Release);
    }
}

/// 在 Slint 主线程中接收用户选择结果并派发后台导入任务
fn dispatch_model_import(pending: PendingModelImport, selected: Option<PathBuf>) {
    let Some(win) = pending.win_weak.upgrade() else {
        pending.is_importing.store(false, Ordering::Release);
        return;
    };
    let Some(src_path) = selected else {
        pending.is_importing.store(false, Ordering::Release);
        win.set_global_status(SharedString::from(format!(
            "已取消选择 {} 模型",
            pending.kind.label()
        )));
        return;
    };
    win.set_global_status_is_error(false);
    win.set_global_status(SharedString::from(format!(
        "正在后台导入 {} 模型，请稍候...",
        pending.kind.label()
    )));

    let flag_fallback = pending.is_importing.clone();
    let thread_name = format!("{}-model-importer", pending.kind.label().to_lowercase());
    if let Err(e) = crate::task_runner::global_task_runner().spawn_named(&thread_name, move || {
        run_model_import_worker(pending, src_path)
    }) {
        flag_fallback.store(false, Ordering::Release);
        log::warn!("[偏好设置 警告] 派发模型导入后台线程失败: {}", e);
        win.set_global_status_is_error(true);
        win.set_global_status(SharedString::from(format!("启动模型导入线程失败: {}", e)));
    }
}

/// 后台线程体：按导入种类完成模型文件物理复制并回投主线程回填结果
fn run_model_import_worker(pending: PendingModelImport, src_path: PathBuf) {
    let _guard = TaskRunningGuard(pending.is_importing.clone());
    let imported_opt = match pending.kind {
        ModelImportKind::AsrPackage => waylan_voice::import_asr_model_from_dir(&src_path),
        ModelImportKind::Onnx(sub) => waylan_voice::import_onnx_model_from_file(sub, &src_path),
    };
    drop(_guard);
    let _ = slint::invoke_from_event_loop(move || {
        report_model_import_result(pending, imported_opt);
    });
}

/// 在主线程升级窗口并把导入结果交由界面回填处理器消费
fn report_model_import_result(pending: PendingModelImport, imported_opt: Option<PathBuf>) {
    let Some(win) = pending.win_weak.upgrade() else {
        return;
    };
    handle_imported_model_event(ImportedModelEventParams {
        win: &win,
        cfg_cell: &pending.cfg_cell,
        kind: pending.kind,
        imported_opt,
        apply_fn: pending.apply_fn,
    });
}

/// 模型导入结果事件处理参数
///
/// 承载后台导入线程回传的导入结果、导入种类、共享配置单元与配置写入回调。
struct ImportedModelEventParams<'a> {
    /// 界面窗口引用
    win: &'a SettingsWindow,
    /// 共享配置单元
    cfg_cell: &'a Arc<Mutex<AppConfig>>,
    /// 导入种类
    kind: ModelImportKind,
    /// 后台导入线程的处理结果（None 表示导入失败）
    imported_opt: Option<PathBuf>,
    /// 导入成功后将目标路径写入配置的回调
    apply_fn: ApplyConfigFn,
}

/// 在 Slint 主线程中处理模型导入结果并原子更新配置与界面状态
fn handle_imported_model_event(params: ImportedModelEventParams<'_>) {
    let ImportedModelEventParams {
        win,
        cfg_cell,
        kind,
        imported_opt,
        apply_fn,
    } = params;
    let label = kind.label();
    match imported_opt {
        Some(imported_path) => {
            let name = imported_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let path_str = imported_path.to_string_lossy().to_string();
            let fail_prefix = format!("保存 {} 模型配置失败", label);
            let _ = commit_config_transaction(win, cfg_cell, &fail_prefix, true, |cfg| {
                update_config_from_window(win, cfg);
                apply_fn(cfg, &path_str);
                Ok(Some(format!("成功导入 {} 模型: {}", label, name)))
            });
        }
        None => {
            win.set_global_status_is_error(true);
            win.set_global_status(SharedString::from(kind.fail_hint()));
        }
    }
}
