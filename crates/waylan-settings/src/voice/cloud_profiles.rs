//! 云端语音识别 (Whisper/OpenAI 兼容) 服务 Profile 配置管理与连通性测试
//!
//! # 设计原理
//! - **实现初衷**：为用户管理多个云端语音端点（OpenAI、自建 vLLM/Faster-Whisper、Groq 等）提供增删改查与测试。
//! - **核心优势**：连通性测试派发至后台独立线程并具备防重入保护，接口故障反馈直观安全。
//! - **代价与局限**：测试连通性需要访问外部网络。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;
use waylan_voice::{CloudAsrConfig, CloudProfile, CloudProviderType, VoiceEngineType};

use crate::SettingsWindow;
use crate::callbacks::TaskRunningGuard;
use crate::sync::update_config_from_window;

/// 云端服务回调共享上下文
///
/// # 设计原理
/// - **实现初衷**：新增、编辑、取消、保存与删除五类回调都要同时持有窗口弱引用、共享配置单元
///   与当前编辑条目标识，逐回调重复展开会让每个闭包体都退化成多参数转发行。
/// - **核心优势**：以单一可克隆上下文承载稳定依赖，闭包仅需一次具名函数转发。
/// - **代价与局限**：上下文随每个回调注册点克隆一份（均为 `Rc` / `Arc` 级轻量句柄）。
struct CloudProfileCallbackContext {
    /// 窗口弱引用（升级失败表示窗口已销毁，回调应静默跳过）
    window_weak: slint::Weak<SettingsWindow>,
    /// 共享配置单元
    cfg_cell: Arc<Mutex<AppConfig>>,
    /// 当前处于编辑态的云端服务条目 ID（None 表示新增态）
    editing_id: RefCell<Option<String>>,
}

/// 注册云端语音服务管理相关回调（新增、编辑、删除、保存、取消、连接测试）
pub(crate) fn register_cloud_profile_callbacks(
    window: &SettingsWindow,
    loaded_config: Arc<Mutex<AppConfig>>,
) {
    let context = Rc::new(CloudProfileCallbackContext {
        window_weak: window.as_weak(),
        cfg_cell: Arc::clone(&loaded_config),
        editing_id: RefCell::new(None),
    });

    let add_ctx = Rc::clone(&context);
    window.on_start_add_cloud_profile(move || {
        apply_start_add_cloud_profile(&add_ctx);
    });

    let edit_ctx = Rc::clone(&context);
    window.on_start_edit_cloud_profile(move |idx| {
        apply_start_edit_cloud_profile(&edit_ctx, idx);
    });

    let cancel_ctx = Rc::clone(&context);
    window.on_cancel_cloud_edit(move || {
        apply_cancel_cloud_edit(&cancel_ctx);
    });

    let save_ctx = Rc::clone(&context);
    window.on_save_cloud_profile(move |name, endpoint, key, model| {
        let form = CloudProfileFormData {
            name: &name,
            endpoint: &endpoint,
            key: &key,
            model: &model,
        };
        apply_save_cloud_profile(&save_ctx, form);
    });

    window.on_delete_cloud_profile(move |idx| {
        apply_delete_cloud_profile(&context, idx);
    });

    // 连通性测试具备独立的防重入标志与后台线程，保持单独注册
    register_cloud_test_callback(window, loaded_config);
}

/// 装配新增态云端服务表单初值并进入编辑模式
///
/// 新增态的表单初值属于纯界面装配逻辑，与配置持久化无关，故不触碰共享配置单元。
fn apply_start_add_cloud_profile(ctx: &CloudProfileCallbackContext) {
    let Some(win) = ctx.window_weak.upgrade() else {
        return;
    };
    ctx.editing_id.borrow_mut().take();
    win.set_edit_cloud_name(SharedString::from(""));
    win.set_edit_cloud_endpoint(SharedString::from(
        "https://api.openai.com/v1/audio/transcriptions",
    ));
    win.set_edit_cloud_key(SharedString::from(""));
    win.set_edit_cloud_model(SharedString::from("whisper-1"));
    win.set_is_editing_cloud(true);
    win.set_global_status(SharedString::from("正在添加新的云端语音服务配置..."));
}

/// 从共享配置回读指定条目并装配编辑态表单
fn apply_start_edit_cloud_profile(ctx: &CloudProfileCallbackContext, idx: i32) {
    let Some(win) = ctx.window_weak.upgrade() else {
        return;
    };
    let guard = ctx.cfg_cell.lock();
    let Some(profile) = guard.voice.cloud_profiles.get(idx as usize) else {
        return;
    };
    *ctx.editing_id.borrow_mut() = Some(profile.id.clone());
    win.set_edit_cloud_name(SharedString::from(&profile.name));
    win.set_edit_cloud_endpoint(SharedString::from(&profile.endpoint));
    win.set_edit_cloud_key(SharedString::from(profile.api_key.as_deref().unwrap_or("")));
    win.set_edit_cloud_model(SharedString::from(&profile.model));
    win.set_is_editing_cloud(true);
    win.set_global_status(SharedString::from(format!(
        "正在编辑云端服务: {}",
        profile.name
    )));
}

/// 取消云端服务编辑并清空当前编辑标识
fn apply_cancel_cloud_edit(ctx: &CloudProfileCallbackContext) {
    let Some(win) = ctx.window_weak.upgrade() else {
        return;
    };
    ctx.editing_id.borrow_mut().take();
    win.set_is_editing_cloud(false);
    win.set_global_status(SharedString::from("已取消云端服务编辑。"));
}

/// 校验并落盘云端服务表单（新增或覆盖当前编辑条目）
fn apply_save_cloud_profile(ctx: &CloudProfileCallbackContext, form: CloudProfileFormData<'_>) {
    let Some(win) = ctx.window_weak.upgrade() else {
        return;
    };
    handle_save_cloud_profile(&win, &ctx.cfg_cell, &ctx.editing_id, form);
}

/// 删除指定索引的云端服务条目
fn apply_delete_cloud_profile(ctx: &CloudProfileCallbackContext, idx: i32) {
    let Some(win) = ctx.window_weak.upgrade() else {
        return;
    };
    handle_delete_cloud_profile(&win, &ctx.cfg_cell, &ctx.editing_id, idx as usize);
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
fn apply_cloud_profile_update(
    cfg: &mut AppConfig,
    target_id: &str,
    values: CloudProfileValues,
) -> String {
    let display_name = values.name.clone();
    let is_active = cfg
        .voice
        .selected_asr
        .as_deref()
        .and_then(|s| s.strip_prefix("cloud:"))
        == Some(target_id);
    if is_active {
        cfg.voice.cloud.endpoint = values.endpoint.clone();
        cfg.voice.cloud.api_key = values.api_key.clone();
        cfg.voice.cloud.model = values.model.clone();
    }
    if let Some(p) = cfg
        .voice
        .cloud_profiles
        .iter_mut()
        .find(|p| p.id == target_id)
    {
        p.name = values.name;
        p.endpoint = values.endpoint;
        p.api_key = values.api_key;
        p.model = values.model;
    }
    display_name
}

static PROFILE_ID_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 向配置中追加一个新的云端语音服务条目
fn apply_cloud_profile_insert(cfg: &mut AppConfig, values: CloudProfileValues) -> String {
    let display_name = values.name.clone();
    let seq = PROFILE_ID_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let new_id = format!("cloud_{}_{}", chrono::Utc::now().timestamp_millis(), seq);
    cfg.voice.cloud_profiles.push(CloudProfile {
        id: new_id,
        name: values.name,
        provider: CloudProviderType::OpenAiCompatible,
        endpoint: values.endpoint,
        api_key: values.api_key,
        model: values.model,
    });
    display_name
}

/// 统一执行配置修改、磁盘持久化、内存回写与窗口同步事务
///
/// 复用全局 `commit_config_transaction` 持锁完成写盘，防止并发 Lost Update 竞态。
fn commit_config_change<F>(
    win: &SettingsWindow,
    cfg_cell: &Arc<Mutex<AppConfig>>,
    error_prefix: &str,
    mutate: F,
) -> Option<String>
where
    F: FnOnce(&mut AppConfig) -> Option<String>,
{
    let mut result_msg = None;
    super::commit_config_transaction(win, cfg_cell, error_prefix, true, |next_cfg| {
        update_config_from_window(win, next_cfg);
        let msg = mutate(next_cfg);
        result_msg = msg.clone();
        Ok(msg)
    });
    result_msg
}

/// 处理保存/更新云端语音服务配置
fn handle_save_cloud_profile(
    win: &SettingsWindow,
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
    editing_id_save: &RefCell<Option<String>>,
    form: CloudProfileFormData<'_>,
) {
    let Some(values) = normalize_cloud_profile_form(win, form) else {
        return;
    };

    let target_id = editing_id_save.borrow_mut().take();
    let mut values_opt = Some(values);
    let saved = commit_config_change(win, current_cfg_cell, "保存云端服务失败", |cfg| {
        let vals = values_opt.take()?;
        let display_name = if let Some(ref tid) = target_id {
            apply_cloud_profile_update(cfg, tid, vals)
        } else {
            apply_cloud_profile_insert(cfg, vals)
        };
        Some(format!("云端服务 [{}] 已保存生效！", display_name))
    });
    if saved.is_some() {
        win.set_is_editing_cloud(false);
    } else {
        // 保存失败时恢复编辑 ID，避免用户重试时误变成新增条目
        *editing_id_save.borrow_mut() = target_id;
    }
}

/// 处理删除指定云端语音服务配置
fn handle_delete_cloud_profile(
    win: &SettingsWindow,
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
    editing_id: &RefCell<Option<String>>,
    index: usize,
) {
    let deleted = commit_config_change(win, current_cfg_cell, "保存删除状态失败", |cfg| {
        if index >= cfg.voice.cloud_profiles.len() {
            return None;
        }
        let removed = cfg.voice.cloud_profiles.remove(index);
        if cfg
            .voice
            .selected_asr
            .as_deref()
            .and_then(|s| s.strip_prefix("cloud:"))
            == Some(removed.id.as_str())
        {
            cfg.voice.selected_asr = None;
            cfg.voice.engine_type = VoiceEngineType::Offline;
        }
        Some(format!("已删除云端服务: {}", removed.name))
    });
    if deleted.is_some() {
        editing_id.borrow_mut().take();
        win.set_is_editing_cloud(false);
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
