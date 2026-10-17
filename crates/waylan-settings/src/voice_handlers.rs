//! 语音输入设置事件处理器（模型导入与云端 Whisper 服务管理）
//!
//! # 设计原理
//! - **实现初衷**：将语音流水线四大阶段的模型导入、目录浏览以及云端多 Profile 增删改查事件与主窗口解耦。
//! - **核心优势**：职责高度内聚，避免设置主窗口代码膨胀，保障模块化清晰度与可测性。
//! - **代价与局限**：事件依赖 Slint 弱引用升级（as_weak），在窗口析构后安全取消执行。

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;
use waylan_core::{CloudAsrConfig, CloudProfile, CloudProviderType};
use waylan_voice::{import_onnx_model_from_file, ModelSubdirectory};

use crate::callbacks::{pick_directory_dialog, pick_file_dialog};
use crate::sync::{sync_config_to_window, update_config_from_window};
use crate::SettingsWindow;

/// 注册所有语音输入相关界面回调事件
pub fn register_voice_callbacks(window: &SettingsWindow, loaded_config: Rc<RefCell<AppConfig>>) {
    register_model_import_callbacks(window, loaded_config.clone());
    register_cloud_profile_callbacks(window, loaded_config);
}

/// 注册 ASR、VAD、标点与降噪模型导入及目录打开事件
fn register_model_import_callbacks(window: &SettingsWindow, loaded_config: Rc<RefCell<AppConfig>>) {
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
    let current_cfg_cell = loaded_config.clone();
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

/// 执行 ASR 复合模型包目录导入
fn handle_import_asr_model(win: &SettingsWindow, current_cfg_cell: &Rc<RefCell<AppConfig>>) {
    let Some(src_path) =
        pick_directory_dialog("请选择包含 ASR 模型的目录 (需包含 encoder/decoder/tokens)")
    else {
        return;
    };
    let Some(imported_path) = waylan_voice::import_asr_model_from_dir(&src_path) else {
        return;
    };
    let name = imported_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut cfg = current_cfg_cell.borrow().clone();
    update_config_from_window(win, &mut cfg);
    cfg.voice.offline.model_dir = Some(imported_path.to_string_lossy().to_string());
    save_config_and_notify_status(
        win,
        current_cfg_cell,
        cfg,
        &format!("成功导入 ASR 模型: {}", name),
        "保存 ASR 模型配置失败",
    );
}

/// 执行单个 ONNX 神经网络模型文件导入并应用配置修改
fn handle_import_onnx_model<F>(
    win: &SettingsWindow,
    current_cfg_cell: &Rc<RefCell<AppConfig>>,
    category: ModelSubdirectory,
    title: &str,
    mut apply_fn: F,
) where
    F: FnMut(&mut AppConfig, &str),
{
    let Some(src_path) = pick_file_dialog(title, "ONNX 神经网络模型 (*.onnx)", "*.onnx")
    else {
        return;
    };
    let Some(imported_path) = import_onnx_model_from_file(category, &src_path) else {
        return;
    };
    let name = imported_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut cfg = current_cfg_cell.borrow().clone();
    update_config_from_window(win, &mut cfg);
    apply_fn(&mut cfg, &imported_path.to_string_lossy());
    save_config_and_notify_status(
        win,
        current_cfg_cell,
        cfg,
        &format!(
            "成功导入 {} 模型: {}",
            category.as_str().to_uppercase(),
            name
        ),
        &format!("保存 {} 模型配置失败", category.as_str().to_uppercase()),
    );
}

/// 保存配置并更新主窗口全局状态栏提示
fn save_config_and_notify_status(
    win: &SettingsWindow,
    current_cfg_cell: &Rc<RefCell<AppConfig>>,
    cfg: AppConfig,
    success_msg: &str,
    fail_prefix: &str,
) {
    match cfg.save_default() {
        Ok(()) => {
            *current_cfg_cell.borrow_mut() = cfg.clone();
            sync_config_to_window(win, &cfg);
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from(success_msg));
        }
        Err(e) => {
            win.set_global_status_is_error(true);
            win.set_global_status(SharedString::from(format!("{}: {}", fail_prefix, e)));
        }
    }
}

/// 注册云端语音服务管理相关回调（新增、编辑、删除、保存、取消、连接测试）
fn register_cloud_profile_callbacks(
    window: &SettingsWindow,
    loaded_config: Rc<RefCell<AppConfig>>,
) {
    let editing_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // 1. 开始新增
    let window_weak = window.as_weak();
    let editing_id_add = editing_id.clone();
    window.on_start_add_cloud_profile(move || {
        if let Some(win) = window_weak.upgrade() {
            editing_id_add.borrow_mut().take();
            win.set_edit_cloud_name(SharedString::from(""));
            win.set_edit_cloud_endpoint(SharedString::from(
                "https://api.openai.com/v1/audio/transcriptions",
            ));
            win.set_edit_cloud_key(SharedString::from(""));
            win.set_edit_cloud_model(SharedString::from("whisper-1"));
            win.set_is_editing_cloud(true);
            win.set_global_status(SharedString::from("正在添加新的云端语音服务配置..."));
        }
    });

    // 2. 开始编辑
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config.clone();
    let editing_id_edit = editing_id.clone();
    window.on_start_edit_cloud_profile(move |idx| {
        if let Some(win) = window_weak.upgrade() {
            let index = idx as usize;
            let cfg = current_cfg_cell.borrow();
            if let Some(p) = cfg.voice.cloud_profiles.get(index) {
                *editing_id_edit.borrow_mut() = Some(p.id.clone());
                win.set_edit_cloud_name(SharedString::from(&p.name));
                win.set_edit_cloud_endpoint(SharedString::from(&p.endpoint));
                win.set_edit_cloud_key(SharedString::from(p.api_key.as_deref().unwrap_or("")));
                win.set_edit_cloud_model(SharedString::from(&p.model));
                win.set_is_editing_cloud(true);
                win.set_global_status(SharedString::from(format!("正在编辑云端服务: {}", p.name)));
            }
        }
    });

    // 3. 取消编辑
    let window_weak = window.as_weak();
    let editing_id_cancel = editing_id.clone();
    window.on_cancel_cloud_edit(move || {
        if let Some(win) = window_weak.upgrade() {
            editing_id_cancel.borrow_mut().take();
            win.set_is_editing_cloud(false);
            win.set_global_status(SharedString::from("已取消云端服务编辑。"));
        }
    });

    // 4. 保存云端服务
    register_cloud_save_and_delete(window, loaded_config.clone(), editing_id);

    // 5. 测试云端服务
    register_cloud_test_callback(window, loaded_config);
}

/// 注册云端服务保存与删除回调
fn register_cloud_save_and_delete(
    window: &SettingsWindow,
    loaded_config: Rc<RefCell<AppConfig>>,
    editing_id: Rc<RefCell<Option<String>>>,
) {
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config.clone();
    let editing_id_save = editing_id.clone();
    window.on_save_cloud_profile(move |name, endpoint, key, model| {
        if let Some(win) = window_weak.upgrade() {
            handle_save_cloud_profile(
                &win,
                &current_cfg_cell,
                &editing_id_save,
                &name,
                &endpoint,
                &key,
                &model,
            );
        }
    });

    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
    window.on_delete_cloud_profile(move |idx| {
        if let Some(win) = window_weak.upgrade() {
            handle_delete_cloud_profile(&win, &current_cfg_cell, &editing_id, idx as usize);
        }
    });
}

/// 处理保存/更新云端语音服务配置
fn handle_save_cloud_profile(
    win: &SettingsWindow,
    current_cfg_cell: &Rc<RefCell<AppConfig>>,
    editing_id_save: &Rc<RefCell<Option<String>>>,
    name: &str,
    endpoint: &str,
    key: &str,
    model: &str,
) {
    let name_str = name.trim().to_string();
    let endpoint_str = endpoint.trim().to_string();
    let model_str = model.trim().to_string();
    if name_str.is_empty() || endpoint_str.is_empty() || model_str.is_empty() {
        win.set_global_status_is_error(true);
        win.set_global_status(SharedString::from("服务名称、接口地址和模型标识不能为空！"));
        return;
    }
    let key_str = key.trim();
    let api_key = if key_str.is_empty() {
        None
    } else {
        Some(key_str.to_string())
    };

    let mut cfg = current_cfg_cell.borrow().clone();
    update_config_from_window(win, &mut cfg);
    let target_id = editing_id_save.borrow_mut().take();
    if let Some(ref tid) = target_id {
        if let Some(p) = cfg.voice.cloud_profiles.iter_mut().find(|p| &p.id == tid) {
            p.name = name_str.clone();
            p.endpoint = endpoint_str.clone();
            p.api_key = api_key.clone();
            p.model = model_str.clone();
        }
        if cfg.voice.selected_asr.as_deref() == Some(&format!("cloud:{}", tid)) {
            cfg.voice.cloud.endpoint = endpoint_str;
            cfg.voice.cloud.api_key = api_key;
            cfg.voice.cloud.model = model_str;
        }
    } else {
        let new_id = format!("cloud_{}", chrono::Utc::now().timestamp_millis());
        cfg.voice.cloud_profiles.push(CloudProfile {
            id: new_id,
            name: name_str.clone(),
            provider: CloudProviderType::OpenAiCompatible,
            endpoint: endpoint_str,
            api_key,
            model: model_str,
        });
    }

    match cfg.save_default() {
        Ok(()) => {
            win.set_is_editing_cloud(false);
            *current_cfg_cell.borrow_mut() = cfg.clone();
            sync_config_to_window(win, &cfg);
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from(format!(
                "云端服务 [{}] 已保存生效！",
                name_str
            )));
        }
        Err(e) => {
            win.set_global_status_is_error(true);
            win.set_global_status(SharedString::from(format!("保存云端服务失败: {}", e)));
        }
    }
}

/// 处理删除指定云端语音服务配置
fn handle_delete_cloud_profile(
    win: &SettingsWindow,
    current_cfg_cell: &Rc<RefCell<AppConfig>>,
    editing_id: &Rc<RefCell<Option<String>>>,
    index: usize,
) {
    let mut cfg = current_cfg_cell.borrow().clone();
    update_config_from_window(win, &mut cfg);
    if index < cfg.voice.cloud_profiles.len() {
        let removed = cfg.voice.cloud_profiles.remove(index);
        if cfg.voice.selected_asr.as_deref() == Some(&format!("cloud:{}", removed.id)) {
            cfg.voice.selected_asr = None;
            cfg.voice.engine_type = waylan_core::VoiceEngineType::Offline;
        }
        editing_id.borrow_mut().take();
        win.set_is_editing_cloud(false);
        match cfg.save_default() {
            Ok(()) => {
                *current_cfg_cell.borrow_mut() = cfg.clone();
                sync_config_to_window(win, &cfg);
                win.set_global_status_is_error(false);
                win.set_global_status(SharedString::from(format!(
                    "已删除云端服务: {}",
                    removed.name
                )));
            }
            Err(e) => {
                win.set_global_status_is_error(true);
                win.set_global_status(SharedString::from(format!("保存删除状态失败: {}", e)));
            }
        }
    }
}

/// 注册云端服务异步连通性测试回调
fn register_cloud_test_callback(window: &SettingsWindow, loaded_config: Rc<RefCell<AppConfig>>) {
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
    window.on_test_cloud_profile(move |idx| {
        if let Some(win) = window_weak.upgrade() {
            let index = idx as usize;
            let cfg = current_cfg_cell.borrow();
            if let Some(p) = cfg.voice.cloud_profiles.get(index) {
                let profile_name = p.name.clone();
                let test_config = CloudAsrConfig {
                    provider: CloudProviderType::OpenAiCompatible,
                    endpoint: p.endpoint.clone(),
                    api_key: p.api_key.clone(),
                    model: p.model.clone(),
                };
                win.set_global_status_is_error(false);
                win.set_global_status(SharedString::from(format!(
                    "正在测试 [{}] 连接，请稍候...",
                    profile_name
                )));
                spawn_cloud_test_task(win.as_weak(), profile_name, test_config);
            }
        }
    });
}

/// 异步执行云端语音服务连通性测试并回写主窗口状态
fn spawn_cloud_test_task(
    win_weak: slint::Weak<SettingsWindow>,
    profile_name: String,
    test_config: CloudAsrConfig,
) {
    std::thread::spawn(move || {
        let (result_msg, is_error) = match waylan_voice::test_cloud_profile_connection(&test_config)
        {
            Ok(resp) => {
                let trimmed = resp.trim();
                if trimmed.is_empty() {
                    (
                        format!("云端服务 [{}] 连接成功！远端接口响应正常。", profile_name),
                        false,
                    )
                } else {
                    (
                        format!(
                            "云端服务 [{}] 连接成功！远端转录返回: \"{}\"",
                            profile_name, trimmed
                        ),
                        false,
                    )
                }
            }
            Err(e) => {
                let detail = match &e {
                    waylan_voice::VoiceError::RecognizerFailed { message, .. } => message.as_str(),
                    _ => &e.to_string(),
                };
                (
                    format!("云端服务 [{}] 连接失败: {}", profile_name, detail),
                    true,
                )
            }
        };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = win_weak.upgrade() {
                w.set_global_status_is_error(is_error);
                w.set_global_status(SharedString::from(result_msg));
            }
        });
    });
}
