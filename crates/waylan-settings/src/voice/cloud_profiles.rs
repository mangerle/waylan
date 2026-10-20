//! 云端语音识别 (Whisper/OpenAI 兼容) 服务 Profile 配置管理与连通性测试
//!
//! # 设计原理
//! - **实现初衷**：为用户管理多个云端语音端点（OpenAI、自建 vLLM/Faster-Whisper、Groq 等）提供增删改查与测试。
//! - **核心优势**：连通性测试派发至后台独立线程并具备防重入保护，接口故障反馈直观安全。
//! - **代价与局限**：测试连通性需要访问外部网络。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;
use waylan_voice::{CloudAsrConfig, CloudProfile, CloudProviderType, VoiceEngineType};

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
    register_cloud_add_callback(window, editing_id.clone());

    // 2. 开始编辑
    register_cloud_edit_callback(window, loaded_config.clone(), editing_id.clone());

    // 3. 取消编辑
    register_cloud_cancel_callback(window, editing_id.clone());

    // 4. 保存云端服务
    register_cloud_save_and_delete(window, loaded_config.clone(), editing_id);

    // 5. 测试云端服务
    register_cloud_test_callback(window, loaded_config);
}

/// 注册「开始新增云端服务」回调
///
/// 设计意图：新增态的表单初值属于纯界面装配逻辑，与配置持久化无关，故独立成函数，
/// 使主注册函数仅承担回调注册顺序的编排职责。
fn register_cloud_add_callback(window: &SettingsWindow, editing_id: Rc<RefCell<Option<String>>>) {
    let window_weak = window.as_weak();
    window.on_start_add_cloud_profile(move || {
        if let Some(win) = window_weak.upgrade() {
            editing_id.borrow_mut().take();
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
}

/// 注册「开始编辑云端服务」回调
///
/// 设计意图：编辑态需要先从当前生效配置中回读目标条目，故必须持有配置锁的共享引用；
/// 该依赖通过参数显式注入，避免依赖隐式的全局状态。
fn register_cloud_edit_callback(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
    editing_id: Rc<RefCell<Option<String>>>,
) {
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
    window.on_start_edit_cloud_profile(move |idx| {
        if let Some(win) = window_weak.upgrade() {
            let index = idx as usize;
            let cfg = current_cfg_cell.lock().clone();
            if let Some(p) = cfg.voice.cloud_profiles.get(index) {
                *editing_id.borrow_mut() = Some(p.id.clone());
                win.set_edit_cloud_name(SharedString::from(&p.name));
                win.set_edit_cloud_endpoint(SharedString::from(&p.endpoint));
                win.set_edit_cloud_key(SharedString::from(p.api_key.as_deref().unwrap_or("")));
                win.set_edit_cloud_model(SharedString::from(&p.model));
                win.set_is_editing_cloud(true);
                win.set_global_status(SharedString::from(format!("正在编辑云端服务: {}", p.name)));
            }
        }
    });
}

/// 注册「取消编辑云端服务」回调
fn register_cloud_cancel_callback(
    window: &SettingsWindow,
    editing_id: Rc<RefCell<Option<String>>>,
) {
    let window_weak = window.as_weak();
    window.on_cancel_cloud_edit(move || {
        if let Some(win) = window_weak.upgrade() {
            editing_id.borrow_mut().take();
            win.set_is_editing_cloud(false);
            win.set_global_status(SharedString::from("已取消云端服务编辑。"));
        }
    });
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
                CloudProfileFormData {
                    name: &name,
                    endpoint: &endpoint,
                    key: &key,
                    model: &model,
                },
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

/// 云端语音服务表单提交数据
struct CloudProfileFormData<'a> {
    name: &'a str,
    endpoint: &'a str,
    key: &'a str,
    model: &'a str,
}

/// 归一化并校验后的云端语音服务字段值
struct CloudProfileValues {
    name: String,
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

/// 归一化并校验云端语音服务表单输入
///
/// 设计意图：将「去除首尾空白 + 必填项判定 + 空 API Key 归一」这些纯数据加工逻辑从落盘流程中剥离，
/// 使保存主流程专注于「写配置 -> 持久化 -> 回写界面」的顺序编排，校验失败时即刻短路返回。
fn normalize_cloud_profile_form(
    win: &SettingsWindow,
    form: CloudProfileFormData<'_>,
) -> Option<CloudProfileValues> {
    let name = form.name.trim().to_string();
    let endpoint = form.endpoint.trim().to_string();
    let model = form.model.trim().to_string();
    if name.is_empty() || endpoint.is_empty() || model.is_empty() {
        win.set_global_status_is_error(true);
        win.set_global_status(SharedString::from("服务名称、接口地址和模型标识不能为空！"));
        return None;
    }
    let key = form.key.trim();
    let api_key = if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    };
    Some(CloudProfileValues {
        name,
        endpoint,
        model,
        api_key,
    })
}

/// 将归一化后的表单值回写到已存在的云端服务条目，并在其处于生效状态时同步当前云端参数
fn apply_cloud_profile_update(cfg: &mut AppConfig, target_id: &str, values: &CloudProfileValues) {
    if let Some(p) = cfg
        .voice
        .cloud_profiles
        .iter_mut()
        .find(|p| p.id == target_id)
    {
        p.name = values.name.clone();
        p.endpoint = values.endpoint.clone();
        p.api_key = values.api_key.clone();
        p.model = values.model.clone();
    }
    if cfg.voice.selected_asr.as_deref() == Some(&format!("cloud:{}", target_id)) {
        cfg.voice.cloud.endpoint = values.endpoint.clone();
        cfg.voice.cloud.api_key = values.api_key.clone();
        cfg.voice.cloud.model = values.model.clone();
    }
}

/// 向配置中追加一个新的云端语音服务条目
fn apply_cloud_profile_insert(cfg: &mut AppConfig, values: &CloudProfileValues) {
    let new_id = format!("cloud_{}", chrono::Utc::now().timestamp_millis());
    cfg.voice.cloud_profiles.push(CloudProfile {
        id: new_id,
        name: values.name.clone(),
        provider: CloudProviderType::OpenAiCompatible,
        endpoint: values.endpoint.clone(),
        api_key: values.api_key.clone(),
        model: values.model.clone(),
    });
}

/// 处理保存/更新云端语音服务配置
fn handle_save_cloud_profile(
    win: &SettingsWindow,
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
    editing_id_save: &Rc<RefCell<Option<String>>>,
    form: CloudProfileFormData<'_>,
) {
    let Some(values) = normalize_cloud_profile_form(win, form) else {
        return;
    };

    let mut cfg = current_cfg_cell.lock().clone();
    update_config_from_window(win, &mut cfg);
    let target_id = editing_id_save.borrow_mut().take();
    if let Some(ref tid) = target_id {
        apply_cloud_profile_update(&mut cfg, tid, &values);
    } else {
        apply_cloud_profile_insert(&mut cfg, &values);
    }

    match cfg.save_default() {
        Ok(()) => {
            win.set_is_editing_cloud(false);
            *current_cfg_cell.lock() = cfg.clone();
            sync_config_to_window(win, &cfg);
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from(format!(
                "云端服务 [{}] 已保存生效！",
                values.name
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
    let mut cfg = current_cfg_cell.lock().clone();
    update_config_from_window(win, &mut cfg);
    if index < cfg.voice.cloud_profiles.len() {
        let removed = cfg.voice.cloud_profiles.remove(index);
        if cfg.voice.selected_asr.as_deref() == Some(&format!("cloud:{}", removed.id)) {
            cfg.voice.selected_asr = None;
            cfg.voice.engine_type = VoiceEngineType::Offline;
        }
        editing_id.borrow_mut().take();
        win.set_is_editing_cloud(false);
        match cfg.save_default() {
            Ok(()) => {
                *current_cfg_cell.lock() = cfg.clone();
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
    let cfg = current_cfg_cell.lock().clone();
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
