//! 语音模型列表（ASR / VAD / NS 降噪）与 Slint 界面的双向同步映射

use slint::Model;
use waylan_core::config::{AppConfig, UiIndexEnum};
use waylan_voice::{NsEngineType, VadModelType, VoiceEngineType};

use crate::{AsrModelCardItem, SettingsWindow};

/// ASR 模型卡片的 Slint UI 状态码枚举（与 `voice_tab.slint` 约定一一对应）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub(super) enum AsrCardState {
    /// 官方预置模型尚未下载
    NotDownloaded = 0,
    /// 官方预置模型已在本地就绪，待启用
    PresetReady = 2,
    /// 官方预置模型物理存在且当前生效中
    PresetActive = 3,
    /// 本地自定义或云端 Profile 当前生效中
    CustomOrCloudActive = 4,
    /// 本地自定义或云端 Profile 未启用
    CustomOrCloudInactive = 5,
}

impl AsrCardState {
    /// 基于安装与启用标志判定预置模型卡片状态
    #[inline]
    pub(super) fn from_preset(is_installed: bool, is_configured: bool) -> Self {
        match (is_installed, is_configured) {
            (true, true) => Self::PresetActive,
            (true, false) => Self::PresetReady,
            (false, _) => Self::NotDownloaded,
        }
    }

    /// 基于启用标志判定自定义或云端模型卡片状态
    #[inline]
    pub(super) fn from_active(is_active: bool) -> Self {
        if is_active {
            Self::CustomOrCloudActive
        } else {
            Self::CustomOrCloudInactive
        }
    }

    /// 转换为 Slint UI 绑定的状态码
    #[inline]
    pub(super) fn as_raw(self) -> i32 {
        self as i32
    }
}

/// 根据路径或后缀在模型列表中查找匹配项下标
fn find_model_index_by_path(models: &[waylan_voice::ModelItem], path: &str) -> Option<usize> {
    models.iter().position(|m| {
        m.path.as_deref() == Some(path)
            || m.path
                .as_deref()
                .map(|p| p.ends_with(path) || path.ends_with(p))
                .unwrap_or(false)
    })
}

#[inline]
fn default_offline_notice() -> &'static str {
    "当前处于【本地离线模式】: 选用优质端侧离线 ASR 语音模型，所有音频识别与特征计算均在本地设备运行，不消耗公网流量，全面保障隐私与数据安全。"
}

/// 异构 ASR 模型条目列表构建器（依次汇总官方预置、本地自定义与云端服务）
struct AsrModelListBuilder {
    names: Vec<slint::SharedString>,
    items: Vec<AsrModelCardItem>,
    selected_idx: i32,
}

impl AsrModelListBuilder {
    fn new() -> Self {
        Self {
            names: Vec::new(),
            items: Vec::new(),
            selected_idx: 0,
        }
    }

    fn push_entry(&mut self, name: slint::SharedString, item: AsrModelCardItem, is_active: bool) {
        if is_active {
            self.selected_idx = self.items.len() as i32;
        }
        self.names.push(name);
        self.items.push(item);
    }

    fn append_preset_models(&mut self, cfg: &AppConfig, is_cloud_mode: bool) {
        for meta in waylan_voice::PRESET_ASR_MODELS {
            let is_installed = waylan_voice::is_preset_model_installed(meta);
            let is_configured = if is_cloud_mode {
                false
            } else if let Some(dir) = cfg
                .voice
                .offline
                .model_dir
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                std::path::Path::new(dir)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| name == meta.dir_name)
                    || dir.contains(meta.dir_name)
            } else {
                cfg.voice.offline.model_type == meta.model_type && meta.id == "sensevoice-small"
            };

            let state = AsrCardState::from_preset(is_installed, is_configured);
            let badge = if !is_installed && is_configured {
                format!("{} · 待下载", meta.badge)
            } else {
                meta.badge.to_string()
            };

            let dest_path_str = waylan_voice::get_preset_model_dest_dir(meta)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let name = slint::SharedString::from(format!("[官方] {} ({})", meta.name, meta.badge));
            let card = AsrModelCardItem {
                id: slint::SharedString::from(meta.id),
                name: slint::SharedString::from(meta.name),
                badge: slint::SharedString::from(badge),
                size_desc: slint::SharedString::from(meta.size_desc),
                desc: slint::SharedString::from(meta.desc),
                state: state.as_raw(),
                progress: 0.0,
                progress_text: slint::SharedString::from(""),
                is_cloud: false,
                path_or_url: slint::SharedString::from(dest_path_str),
            };
            self.push_entry(name, card, is_configured);
        }
    }

    fn append_local_models(&mut self, cfg: &AppConfig, is_cloud_mode: bool) {
        let local_asrs = waylan_voice::list_asr_models();
        for m in &local_asrs {
            let path_str = m.path.as_deref().unwrap_or_default();
            let is_preset_dir = waylan_voice::PRESET_ASR_MODELS
                .iter()
                .any(|meta| path_str.contains(meta.dir_name));
            if is_preset_dir || m.display_name.contains("[推荐]") {
                continue; // 官方预置项已在第一阶段收敛，避免重复
            }

            let is_active =
                !is_cloud_mode && cfg.voice.offline.model_dir.as_deref() == Some(path_str);
            let state = AsrCardState::from_active(is_active);

            let name = slint::SharedString::from(format!("[本地] {}", m.display_name));
            let card = AsrModelCardItem {
                id: slint::SharedString::from(format!("local:{}", path_str)),
                name: slint::SharedString::from(&m.display_name),
                badge: slint::SharedString::from("本地自定义"),
                size_desc: slint::SharedString::from("本地已导入 · 端侧离线推理"),
                desc: slint::SharedString::from(
                    "用户手动导入或放置在本地共享目录中的自定义 ASR 模型权重文件",
                ),
                state: state.as_raw(),
                progress: 0.0,
                progress_text: slint::SharedString::from(""),
                is_cloud: false,
                path_or_url: slint::SharedString::from(path_str),
            };
            self.push_entry(name, card, is_active);
        }
    }

    fn append_cloud_profiles(&mut self, cfg: &AppConfig, is_cloud_mode: bool) {
        for p in &cfg.voice.cloud_profiles {
            let is_active = is_cloud_mode
                && cfg
                    .voice
                    .selected_asr
                    .as_deref()
                    .and_then(|s| s.strip_prefix("cloud:"))
                    .is_some_and(|id| id == p.id);
            let state = AsrCardState::from_active(is_active);

            let name = slint::SharedString::from(format!("[云端] {} ({})", p.name, p.model));
            let card = AsrModelCardItem {
                id: slint::SharedString::from(format!("cloud:{}", p.id)),
                name: slint::SharedString::from(&p.name),
                badge: slint::SharedString::from("云端服务"),
                size_desc: slint::SharedString::from(format!("云端 Whisper · 模型: {}", p.model)),
                desc: slint::SharedString::from(format!(
                    "说话结束后将音频打包发送至目标云端 API ({})，支持超大模型高精转写",
                    p.endpoint
                )),
                state: state.as_raw(),
                progress: 0.0,
                progress_text: slint::SharedString::from(""),
                is_cloud: true,
                path_or_url: slint::SharedString::from(&p.endpoint),
            };
            self.push_entry(name, card, is_active);
        }
    }
}

/// 同步 ASR 模型统一大列表（官方推荐离线模型 + 本地自定义导入模型 + 线上云端 Profile）至界面
pub(crate) fn sync_asr_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    let is_cloud_mode = cfg.voice.engine_type == VoiceEngineType::Cloud
        || cfg
            .voice
            .selected_asr
            .as_deref()
            .is_some_and(|s| s.starts_with("cloud:"));

    let mut builder = AsrModelListBuilder::new();
    builder.append_preset_models(cfg, is_cloud_mode);
    builder.append_local_models(cfg, is_cloud_mode);
    builder.append_cloud_profiles(cfg, is_cloud_mode);

    let notice = if is_cloud_mode {
        "当前处于【云端服务模式】: 说话结束后将音频打包发送至已配置的远端 Whisper API 处理，依赖公网连接。"
    } else {
        default_offline_notice()
    };

    window.set_asr_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(builder.names),
    )));
    window.set_asr_models(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(builder.items),
    )));
    window.set_asr_model_index(builder.selected_idx);
    window.set_asr_notice_text(slint::SharedString::from(notice));
    window.set_is_asr_cloud(is_cloud_mode);
}

/// 同步 VAD 端点检测模型下拉列表与选中索引
fn sync_vad_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    let vad_models = waylan_voice::list_vad_models();
    let vad_names: Vec<slint::SharedString> = vad_models
        .iter()
        .map(|m| slint::SharedString::from(&m.display_name))
        .collect();
    let vad_idx = if cfg.voice.vad.vad_type == VadModelType::Energy {
        vad_models
            .iter()
            .position(|m| m.path.as_deref() == Some("__energy__"))
            .unwrap_or(0)
    } else if let Some(ref path) = cfg.voice.vad.model_path {
        find_model_index_by_path(&vad_models, path).unwrap_or(0)
    } else {
        0
    };
    window.set_vad_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(vad_names),
    )));
    window.set_vad_model_index(vad_idx as i32);
}

/// 同步人声智能降噪 (NS) 模型下拉列表与选中索引
fn sync_ns_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    let ns_models = waylan_voice::list_ns_models();
    let ns_names: Vec<slint::SharedString> = ns_models
        .iter()
        .filter(|m| m.path.as_deref() != Some("__bypass__"))
        .map(|m| slint::SharedString::from(&m.display_name))
        .collect();
    let ns_idx = cfg.voice.afe.ns_engine.as_ui_index();
    window.set_ns_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(ns_names),
    )));
    window.set_ns_model_index(ns_idx);
}

/// 同步语音三大模型（ASR / VAD / NS 降噪）列表与当前选中项至界面
pub(super) fn sync_voice_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    sync_asr_models_to_window(window, cfg);
    sync_vad_models_to_window(window, cfg);
    sync_ns_models_to_window(window, cfg);
}

/// 从 Slint 界面属性读取模型选择并持久化至配置
pub(super) fn update_voice_models_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    update_asr_model_from_window(window, cfg);
    update_vad_model_from_window(window, cfg);
    update_ns_model_from_window(window, cfg);
}

/// 依据界面统一模型列表中的当前索引同步 ASR 识别引擎配置
fn update_asr_model_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    let Ok(asr_idx) = usize::try_from(window.get_asr_model_index()) else {
        return;
    };
    let models_rc = window.get_asr_models();
    let Some(item) = models_rc.row_data(asr_idx) else {
        return;
    };
    // 仅允许写回已安装到本地的预置模型、本地自定义目录或云端档案，禁止把未下载的预置模型写入配置
    let _ = crate::voice::apply_asr_model_selection(cfg, item.id.as_str(), true);
}

/// 依据界面下拉索引同步 VAD 端点检测模型类型与模型路径
fn update_vad_model_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    let vad_models = waylan_voice::list_vad_models();
    let Ok(vad_idx) = usize::try_from(window.get_vad_model_index()) else {
        return;
    };
    if let Some(m) = vad_models.get(vad_idx) {
        if m.path.as_deref() == Some("__energy__") {
            cfg.voice.vad.vad_type = VadModelType::Energy;
            cfg.voice.vad.model_path = None;
        } else {
            cfg.voice.vad.vad_type = VadModelType::SileroOnnx;
            cfg.voice.vad.model_path = m.path.clone();
        }
    }
}

/// 依据界面下拉索引同步人声智能降噪 (NS) 引擎类型
fn update_ns_model_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    let ns_models = waylan_voice::list_ns_models();
    let ns_filtered: Vec<_> = ns_models
        .into_iter()
        .filter(|m| m.path.as_deref() != Some("__bypass__"))
        .collect();
    let Ok(ns_idx) = usize::try_from(window.get_ns_model_index()) else {
        return;
    };
    if let Some(m) = ns_filtered.get(ns_idx) {
        cfg.voice.afe.ns_engine = match m.path.as_deref() {
            Some("__deepfilternet__") => NsEngineType::DeepFilterNet,
            Some("__gtcrn__") => NsEngineType::Gtcrn,
            _ => NsEngineType::Rnnoise,
        };
    }
}
