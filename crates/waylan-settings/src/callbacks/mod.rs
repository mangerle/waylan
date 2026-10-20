//! 偏好设置窗口业务回调注册与装配
//!
//! # 设计原理
//! - **实现初衷**：收敛短语增删、词库异步导入挂载、配置持久化与桌面 GNOME 指示器同步等 UI 回调注册。
//! - **核心优势**：按职责领域裂变为子模块（desktop / dict / phrases / tasks），本模块仅保留轻量高层回调装配，复杂度严格受控。
//! - **代价与局限**：回调函数需捕获 Slint 窗口的 Weak 引用进行跨线程安全更新。

pub mod desktop;
pub mod dict;
pub mod phrases;
pub mod tasks;

pub use desktop::{
    pick_directory_dialog, pick_directory_dialog_async, pick_file_dialog, pick_file_dialog_async,
    sync_gnome_indicator_mode,
};
pub use dict::{populate_lexicon_statistics, LexiconStatsBundle};
pub(crate) use tasks::TaskRunningGuard;

use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, SharedString};
use waylan_core::config::AppConfig;

use crate::sync::{sync_config_to_window, update_config_from_window};
use crate::SettingsWindow;
use dict::register_dict_import_callbacks;
use phrases::register_phrase_callbacks;

/// 偏好设置回调注册上下文
pub struct SettingsCallbacksContext {
    /// 全局持久化应用配置原子锁
    pub loaded_config: Arc<Mutex<AppConfig>>,
    /// 词库与短语模型集合
    pub stats: LexiconStatsBundle,
}

/// 注册通用配置、自定义短语与外部词库导入回调事件
pub fn register_settings_callbacks(window: &SettingsWindow, context: SettingsCallbacksContext) {
    let SettingsCallbacksContext {
        loaded_config,
        stats:
            LexiconStatsBundle {
                aux_dict,
                phrases_model,
                phrase_path,
            },
    } = context;

    // 1. 注册配置保存与恢复默认设置回调
    register_persistence_callbacks(window, loaded_config);

    // 2. 注册外部词库文件选取与异步导入回调
    register_dict_import_callbacks(window, aux_dict);

    // 3. 注册自定义短语管理与自适应模型清空回调
    register_phrase_callbacks(window, phrases_model, phrase_path);
}

/// 保存当前界面表单到磁盘配置并回写界面状态
///
/// # 设计原理
/// - **实现初衷**：保存链路含"表单回读 -> 原子写盘 -> 共享配置替换 -> GNOME 指示器模式同步 -> 状态回写"，
///   内联在 Slint 事件闭包内会让写盘失败分支与共享状态替换难以核对，故提取为具名函数。
/// - **核心优势**：仅在 `save_default()` 成功后才替换共享配置与同步托盘，失败时保留原配置避免半提交。
/// - **代价与局限**：窗口已销毁时整条链路静默跳过。
fn apply_save_settings(
    window_weak: &slint::Weak<SettingsWindow>,
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
) {
    let Some(win) = window_weak.upgrade() else {
        return;
    };
    let mut base_cfg = current_cfg_cell.lock().clone();
    update_config_from_window(&win, &mut base_cfg);
    match base_cfg.save_default() {
        Ok(()) => {
            let hide_ext = base_cfg.desktop.exclusive_input_source;
            *current_cfg_cell.lock() = base_cfg;
            sync_gnome_indicator_mode(hide_ext);
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from("配置已成功保存并同步！"));
        }
        Err(e) => {
            win.set_global_status_is_error(true);
            win.set_global_status(SharedString::from(format!("保存失败: {}", e)));
        }
    }
}

/// 恢复默认配置到界面表单（不落盘，需用户随后点击保存）
fn apply_reset_defaults(window_weak: &slint::Weak<SettingsWindow>) {
    let Some(win) = window_weak.upgrade() else {
        return;
    };
    let default_cfg = AppConfig::default();
    sync_config_to_window(&win, &default_cfg);
    win.set_global_status_is_error(false);
    win.set_global_status(SharedString::from("已恢复为默认配置（点击保存后生效）"));
}

/// 注册配置持久化回调（保存设置、恢复默认设置）
///
/// 设计意图：将「写盘落盘 + 界面状态回写」这两段具体业务逻辑从主装配函数中剥离，
/// 使主装配函数仅保留回调注册顺序的编排职责，避免装配层与业务实现相互渗透。
fn register_persistence_callbacks(window: &SettingsWindow, loaded_config: Arc<Mutex<AppConfig>>) {
    // 1. 保存设置回调
    let window_weak = window.as_weak();
    let current_cfg_cell = loaded_config;
    window.on_save_settings(move || {
        apply_save_settings(&window_weak, &current_cfg_cell);
    });

    // 2. 恢复默认设置回调
    let window_weak = window.as_weak();
    window.on_reset_defaults(move || {
        apply_reset_defaults(&window_weak);
    });
}
