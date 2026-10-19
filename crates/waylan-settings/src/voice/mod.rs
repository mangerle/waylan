//! 语音输入设置事件处理器模块（本地离线模型导入与云端 Whisper 服务管理）
//!
//! # 设计原理
//! - **实现初衷**：将语音流水线四大阶段的模型导入、目录浏览以及云端多 Profile 增删改查事件与主窗口解耦。
//! - **核心优势**：按关注点分离为 `model_import` 与 `cloud_profiles`，主模块仅保留统一注册入口，复杂度严格受控。
//! - **代价与局限**：事件依赖 Slint 弱引用升级（as_weak），在窗口析构后安全取消执行。

pub mod cloud_profiles;
pub mod model_import;

use std::sync::Arc;

use parking_lot::Mutex;
use slint::SharedString;
use waylan_core::config::AppConfig;

use crate::sync::sync_config_to_window;
use crate::SettingsWindow;

/// 注册所有语音输入相关界面回调事件
pub fn register_voice_callbacks(window: &SettingsWindow, loaded_config: Arc<Mutex<AppConfig>>) {
    model_import::register_model_import_callbacks(window, loaded_config.clone());
    cloud_profiles::register_cloud_profile_callbacks(window, loaded_config);
}

/// 保存配置并更新主窗口全局状态栏提示的参数对象
pub(crate) struct SaveConfigParams<'a> {
    pub win: &'a SettingsWindow,
    pub current_cfg_cell: &'a Arc<Mutex<AppConfig>>,
    pub cfg: AppConfig,
    pub success_msg: &'a str,
    pub fail_prefix: &'a str,
}

/// 保存配置并更新主窗口全局状态栏提示（跨子模块复用）
pub(crate) fn save_config_and_notify_status(params: SaveConfigParams<'_>) {
    let SaveConfigParams {
        win,
        current_cfg_cell,
        cfg,
        success_msg,
        fail_prefix,
    } = params;
    match cfg.save_default() {
        Ok(()) => {
            *current_cfg_cell.lock() = cfg.clone();
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
