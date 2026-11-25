//! 语音输入设置事件处理器模块（本地离线模型导入与云端 Whisper 服务管理）
//!
//! # 设计原理
//! - **实现初衷**：将语音流水线四大阶段的模型导入、目录浏览以及云端多 Profile 增删改查事件与主窗口解耦。
//! - **核心优势**：按关注点分离为 `model_import` 与 `cloud_profiles`，主模块仅保留统一注册入口，复杂度严格受控。
//! - **代价与局限**：事件依赖 Slint 弱引用升级（as_weak），在窗口析构后安全取消执行。

pub mod cloud_profiles;
pub mod model_download;
pub mod model_import;

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::SharedString;
use waylan_core::config::AppConfig;
use waylan_core::{CloudAsrConfig, VoiceEngineType};
use waylan_voice::{
    find_preset_model_by_id, get_preset_model_dest_dir, infer_offline_model_type,
    is_preset_model_installed,
};

use crate::SettingsWindow;
use crate::sync::sync_config_to_window;

/// 注册所有语音输入相关界面回调事件
pub fn register_voice_callbacks(window: &SettingsWindow, loaded_config: Arc<Mutex<AppConfig>>) {
    model_import::register_model_import_callbacks(window, loaded_config.clone());
    model_download::register_model_download_callbacks(window, loaded_config.clone());
    cloud_profiles::register_cloud_profile_callbacks(window, loaded_config);
}

/// 持锁原子执行配置变更、磁盘落盘与状态栏反馈（消除并发 Lost Update 竞态）
pub(crate) fn commit_config_transaction<F>(
    win: &SettingsWindow,
    current_cfg_cell: &Arc<Mutex<AppConfig>>,
    fail_prefix: &str,
    sync_ui: bool,
    mutate: F,
) -> Option<AppConfig>
where
    F: FnOnce(&mut AppConfig) -> Result<Option<String>, String>,
{
    let mut guard = current_cfg_cell.lock();
    let mut next_cfg = guard.clone();
    let success_msg = match mutate(&mut next_cfg) {
        Ok(Some(msg)) => msg,
        Ok(None) => return None,
        Err(err_msg) => {
            win.set_global_status_is_error(true);
            win.set_global_status(SharedString::from(err_msg));
            return None;
        }
    };

    match next_cfg.save_default() {
        Ok(()) => {
            *guard = next_cfg.clone();
            drop(guard);
            if sync_ui {
                sync_config_to_window(win, &next_cfg);
            }
            win.set_global_status_is_error(false);
            win.set_global_status(SharedString::from(success_msg));
            Some(next_cfg)
        }
        Err(e) => {
            win.set_global_status_is_error(true);
            win.set_global_status(SharedString::from(format!("{}: {}", fail_prefix, e)));
            None
        }
    }
}

/// 统一根据模型标识（预置 / `local:` / `cloud:`）切换 ASR 配置字段
pub(crate) fn apply_asr_model_selection(
    cfg: &mut AppConfig,
    model_id: &str,
    require_installed: bool,
) -> Result<Option<String>, String> {
    if let Some(meta) = find_preset_model_by_id(model_id) {
        if require_installed && !is_preset_model_installed(meta) {
            return Err(format!(
                "模型 [{}] 尚未在本地下载，请先点击【下载并部署】",
                meta.name
            ));
        }
        cfg.voice.engine_type = VoiceEngineType::Offline;
        cfg.voice.selected_asr = None;
        cfg.voice.offline.model_type = meta.model_type;
        if let Some(dest_dir) = get_preset_model_dest_dir(meta)
            && (!require_installed || is_preset_model_installed(meta))
        {
            cfg.voice.offline.model_dir = Some(dest_dir.to_string_lossy().to_string());
        }
        return Ok(Some(format!(
            "已成功启用 [{}] 离线语音识别引擎，配置已落盘热重载生效",
            meta.name
        )));
    }
    if let Some(local_path) = model_id.strip_prefix("local:") {
        cfg.voice.engine_type = VoiceEngineType::Offline;
        cfg.voice.selected_asr = None;
        cfg.voice.offline.model_dir = Some(local_path.to_string());
        cfg.voice.offline.model_type = infer_offline_model_type(Path::new(local_path));
        return Ok(Some(
            "已成功切换至本地自定义 ASR 模型，配置已落盘热重载生效".to_string(),
        ));
    }
    if let Some(cloud_id) = model_id.strip_prefix("cloud:") {
        let Some(profile) = cfg
            .voice
            .cloud_profiles
            .iter()
            .find(|p| p.id == cloud_id)
            .cloned()
        else {
            return Ok(None);
        };
        cfg.voice.engine_type = VoiceEngineType::Cloud;
        cfg.voice.selected_asr = Some(format!("cloud:{}", cloud_id));
        cfg.voice.cloud = CloudAsrConfig::from(&profile);
        return Ok(Some(format!(
            "已成功切换至 [{}] 云端转写模式，配置已落盘生效",
            profile.name
        )));
    }
    Ok(None)
}
