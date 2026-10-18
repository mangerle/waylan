//! 云端语音识别 (Whisper/OpenAI 兼容) 服务 Profile 配置管理与连通性测试
//!
//! # 设计原理
//! - **实现初衷**：为用户管理多个云端语音端点（OpenAI、自建 vLLM/Faster-Whisper、Groq 等）提供增删改查与测试。
//! - **核心优势**：连通性测试派发至后台独立线程并具备防重入保护，接口故障反馈直观安全。
//! - **代价与局限**：测试连通性需要访问外部网络。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;
use waylan_core::{CloudAsrConfig, CloudProfile, CloudProviderType};

use crate::callbacks::TaskRunningGuard;
use crate::sync::{sync_config_to_window, update_config_from_window};
use crate::SettingsWindow;

/// 注册云端语音服务管理相关回调（新增、编辑、删除、保存、取消、连接测试）
pub(crate) fn register_cloud_profile_callbacks(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
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
            let cfg = current_cfg_cell
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
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
    loaded_config: Arc<Mutex<AppConfig>>,
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
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
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

    let mut cfg = current_cfg_cell
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
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
            if let Ok(mut g) = current_cfg_cell.lock() {
                *g = cfg.clone();
            }
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
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
    editing_id: &Rc<RefCell<Option<String>>>,
    index: usize,
) {
    let mut cfg = current_cfg_cell
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
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
                if let Ok(mut g) = current_cfg_cell.lock() {
                    *g = cfg.clone();
                }
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

/// 处理单个云端服务测试请求的具体业务逻辑
fn handle_test_cloud_profile(
    win: &SettingsWindow,
    idx: i32,
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
    testing_flag: &Arc<AtomicBool>,
) {
    if testing_flag
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        win.set_global_status_is_error(false);
        win.set_global_status(SharedString::from(
            "已有云端连接测试任务正在执行，请稍候...",
        ));
        return;
    }
    let index = idx as usize;
    let cfg = current_cfg_cell
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
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
        spawn_cloud_test_task(
            win.as_weak(),
            profile_name,
            test_config,
            testing_flag.clone(),
        );
    } else {
        testing_flag.store(false, Ordering::Release);
    }
}

/// 注册云端服务异步连通性测试回调
fn register_cloud_test_callback(window: &SettingsWindow, loaded_config: Arc<Mutex<AppConfig>>) {
    let is_testing = Arc::new(AtomicBool::new(false));
    let testing_flag = is_testing.clone();
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
    window.on_test_cloud_profile(move |idx| {
        if let Some(win) = window_weak.upgrade() {
            handle_test_cloud_profile(&win, idx, &current_cfg_cell, &testing_flag);
        }
    });
}

/// 格式化云端连接测试输出结果为用户界面状态文本
fn format_cloud_test_result(profile_name: &str, test_config: &CloudAsrConfig) -> (String, bool) {
    match waylan_voice::test_cloud_profile_connection(test_config) {
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
    }
}

/// 在专用后台线程中执行云端测试并向 Slint 事件循环回发结果
fn execute_cloud_test_thread(
    win_weak: slint::Weak<SettingsWindow>,
    profile_name: String,
    test_config: CloudAsrConfig,
    guard_flag: Arc<AtomicBool>,
) {
    let _guard = TaskRunningGuard(guard_flag);
    let (result_msg, is_error) = format_cloud_test_result(&profile_name, &test_config);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = win_weak.upgrade() {
            w.set_global_status_is_error(is_error);
            w.set_global_status(SharedString::from(result_msg));
        }
    });
}

/// 异步执行云端语音服务连通性测试并回写主窗口状态
fn spawn_cloud_test_task(
    win_weak: slint::Weak<SettingsWindow>,
    profile_name: String,
    test_config: CloudAsrConfig,
    running_flag: Arc<AtomicBool>,
) {
    let guard_flag = running_flag.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("cloud-profile-tester".to_string())
        .spawn(move || execute_cloud_test_thread(win_weak, profile_name, test_config, guard_flag))
    {
        running_flag.store(false, Ordering::Release);
        log::warn!("[偏好设置 警告] 派发云端测试线程失败: {}", e);
    }
}
