use crate::{AsrModelCardItem, CloudProfileItem, SettingsWindow};
use slint::Model;
use waylan_core::config::{AppConfig, UiIndexEnum};
use waylan_voice::{NsEngineType, VadModelType, VoiceEngineType, VoiceVisualEffect};

/// 对敏感 API Key 做字符级脱敏，避免按字节截断多字节字符导致 panic
fn mask_api_key(key: &str) -> String {
    if key.chars().count() > 6 {
        let prefix: String = key.chars().take(4).collect();
        format!("{}******", prefix)
    } else {
        "******".to_string()
    }
}

/// 同步语音输入开关、快捷键与各处理阶段开关至 Slint 界面属性
fn sync_voice_switches_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    window.set_voice_enabled(cfg.voice.enabled);

    // 连续听写开关快捷键 (Toggle)
    let toggle_str = cfg.voice.toggle_shortcut.trim();
    window.set_toggle_shortcut(slint::SharedString::from(toggle_str));

    // 长按对讲机快捷键 (Push-to-Talk)
    let ptt_str = cfg.voice.push_to_talk_shortcut.trim();
    window.set_push_to_talk_shortcut(slint::SharedString::from(ptt_str));

    // 阶段一 AFE 前端增强
    window.set_afe_enabled(cfg.voice.afe.enabled);
    window.set_high_pass_filter(cfg.voice.afe.high_pass_filter);
    window.set_noise_suppression(cfg.voice.afe.noise_suppression);
    window.set_auto_gain_control(cfg.voice.afe.auto_gain_control);

    // 阶段二 VAD 端点截断
    window.set_vad_silence_delay_index(match cfg.voice.vad.silence_timeout_ms {
        0..=500 => 0,
        501..=700 => 1,
        701..=900 => 2,
        _ => 3,
    });

    // 阶段四 标点预测
    window.set_auto_punctuation(cfg.voice.auto_punctuation);

    // 阶段五 文本规整与语气词过滤 (ITN)
    window.set_itn_convert_numbers(cfg.voice.itn.convert_numbers);
    window.set_itn_filter_filler_words(cfg.voice.itn.filter_filler_words);
    window.set_itn_pangu_spacing(cfg.voice.itn.pangu_spacing);

    // 阶段六 交互展现与流式上屏
    window.set_voice_real_time_preview(cfg.voice.interaction.real_time_preview);
    window.set_voice_show_floating_bar(cfg.voice.interaction.show_floating_bar);
    window.set_voice_visual_effect_index(cfg.voice.interaction.visual_effect.as_ui_index());
}

/// 同步云端服务 Profile 列表至 Slint 界面属性
fn sync_cloud_profiles_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    let active_cloud_id = if let Some(ref sel) = cfg.voice.selected_asr {
        sel.strip_prefix("cloud:")
    } else {
        None
    };
    let cloud_items: Vec<CloudProfileItem> = cfg
        .voice
        .cloud_profiles
        .iter()
        .map(|p| CloudProfileItem {
            id: slint::SharedString::from(&p.id),
            name: slint::SharedString::from(&p.name),
            endpoint: slint::SharedString::from(&p.endpoint),
            api_key: slint::SharedString::from(
                p.api_key
                    .as_deref()
                    .map(mask_api_key)
                    .unwrap_or_else(|| "未设置".to_string()),
            ),
            model: slint::SharedString::from(&p.model),
            is_active: active_cloud_id == Some(p.id.as_str()),
        })
        .collect();
    window.set_cloud_profiles(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(cloud_items),
    )));
}

/// 同步语音输入配置与模型状态至 Slint 界面属性
pub(super) fn sync_voice_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    sync_voice_switches_to_window(window, cfg);
    sync_cloud_profiles_to_window(window, cfg);
    sync_voice_models_to_window(window, cfg);
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
fn default_offline_notice() -> String {
    "当前处于【本地离线模式】: 选用优质端侧离线 ASR 语音模型，所有音频识别与特征计算均在本地设备运行，不消耗公网流量，全面保障隐私与数据安全。".to_string()
}

/// ASR 模型卡片的 Slint UI 状态码枚举（与 `voice_tab.slint` 约定一一对应）
#[derive(Debug, Clone, Copy)]
#[repr(i32)]
enum AsrCardState {
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
            let is_configured = !is_cloud_mode
                && (cfg.voice.offline.model_type == meta.model_type
                    || cfg
                        .voice
                        .offline
                        .model_dir
                        .as_deref()
                        .is_some_and(|d| d.contains(meta.dir_name)));

            let state = match (is_installed, is_configured) {
                (true, true) => AsrCardState::PresetActive,
                (true, false) => AsrCardState::PresetReady,
                (false, _) => AsrCardState::NotDownloaded,
            };

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
                state: state as i32,
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
            let state = if is_active {
                AsrCardState::CustomOrCloudActive
            } else {
                AsrCardState::CustomOrCloudInactive
            };

            let name = slint::SharedString::from(format!("[本地] {}", m.display_name));
            let card = AsrModelCardItem {
                id: slint::SharedString::from(format!("local:{}", path_str)),
                name: slint::SharedString::from(&m.display_name),
                badge: slint::SharedString::from("本地自定义"),
                size_desc: slint::SharedString::from("本地已导入 · 端侧离线推理"),
                desc: slint::SharedString::from(
                    "用户手动导入或放置在本地共享目录中的自定义 ASR 模型权重文件",
                ),
                state: state as i32,
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
            let state = if is_active {
                AsrCardState::CustomOrCloudActive
            } else {
                AsrCardState::CustomOrCloudInactive
            };

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
                state: state as i32,
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
        "当前处于【云端服务模式】: 说话结束后将音频打包发送至已配置的远端 Whisper API 处理，依赖公网连接。".to_string()
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

/// 同步语音三大模型（ASR / VAD / NS降噪）列表与当前选中项至界面
fn sync_voice_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    sync_asr_models_to_window(window, cfg);
    sync_vad_models_to_window(window, cfg);
    sync_ns_models_to_window(window, cfg);
}

/// 从 Slint 界面属性增量读取语音输入配置
pub(super) fn update_voice_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    cfg.voice.enabled = window.get_voice_enabled();

    // 连续听写开关快捷键 (Toggle)
    cfg.voice.toggle_shortcut = normalize_shortcut_value(&window.get_toggle_shortcut());

    // 长按对讲机快捷键 (Push-to-Talk)
    cfg.voice.push_to_talk_shortcut = normalize_shortcut_value(&window.get_push_to_talk_shortcut());

    // 阶段一 AFE 前端增强
    cfg.voice.afe.enabled = window.get_afe_enabled();
    cfg.voice.afe.high_pass_filter = window.get_high_pass_filter();
    cfg.voice.afe.noise_suppression = window.get_noise_suppression();
    cfg.voice.afe.auto_gain_control = window.get_auto_gain_control();

    // 阶段二 VAD 端点检测截断时间
    cfg.voice.vad.silence_timeout_ms = match window.get_vad_silence_delay_index() {
        0 => 400,
        2 => 800,
        3 => 1000,
        _ => 600,
    };

    // 阶段四 标点预测
    cfg.voice.auto_punctuation = window.get_auto_punctuation();

    // 阶段五 文本规整与语气词过滤 (ITN)
    cfg.voice.itn.convert_numbers = window.get_itn_convert_numbers();
    cfg.voice.itn.filter_filler_words = window.get_itn_filter_filler_words();
    cfg.voice.itn.pangu_spacing = window.get_itn_pangu_spacing();

    // 阶段六 交互展现与流式上屏
    cfg.voice.interaction.real_time_preview = window.get_voice_real_time_preview();
    cfg.voice.interaction.show_floating_bar = window.get_voice_show_floating_bar();
    cfg.voice.interaction.visual_effect =
        VoiceVisualEffect::from_ui_index(window.get_voice_visual_effect_index());

    update_voice_models_from_window(window, cfg);
}

/// 将界面上的快捷键文本归一化为配置可存储的标准取值
///
/// 设计意图：空值与 `disabled` 两种「未启用」的表述在界面与配置中并不统一，
/// 此处集中收敛为唯一哨兵值「已禁用」，避免同一判定规则散落在多个读取点而产生漂移。
fn normalize_shortcut_value(raw: &str) -> String {
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("disabled") || trimmed == "已禁用" {
        "已禁用".to_string()
    } else {
        trimmed
    }
}

/// 从 Slint 界面属性读取模型选择并持久化至配置
fn update_voice_models_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    update_asr_model_from_window(window, cfg);
    update_vad_model_from_window(window, cfg);
    update_ns_model_from_window(window, cfg);
}

/// 依据界面统一模型列表中的当前索引同步 ASR 识别引擎配置
fn update_asr_model_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    let asr_idx = window.get_asr_model_index() as usize;
    let models_rc = window.get_asr_models();
    let Some(item) = models_rc.row_data(asr_idx) else {
        return;
    };

    let item_id = item.id.as_str();
    if let Some(meta) = waylan_voice::find_preset_model_by_id(item_id) {
        cfg.voice.engine_type = VoiceEngineType::Offline;
        cfg.voice.selected_asr = None;
        cfg.voice.offline.model_type = meta.model_type;
        if waylan_voice::is_preset_model_installed(meta)
            && let Some(dest) = waylan_voice::get_preset_model_dest_dir(meta)
        {
            cfg.voice.offline.model_dir = Some(dest.to_string_lossy().to_string());
        }
    } else if let Some(local_path) = item_id.strip_prefix("local:") {
        cfg.voice.engine_type = VoiceEngineType::Offline;
        cfg.voice.selected_asr = None;
        cfg.voice.offline.model_dir = Some(local_path.to_string());
    } else if let Some(cloud_id) = item_id.strip_prefix("cloud:") {
        cfg.voice.engine_type = VoiceEngineType::Cloud;
        cfg.voice.selected_asr = Some(format!("cloud:{}", cloud_id));
        if let Some(profile) = cfg.voice.cloud_profiles.iter().find(|p| p.id == cloud_id) {
            cfg.voice.cloud.endpoint = profile.endpoint.clone();
            cfg.voice.cloud.api_key = profile.api_key.clone();
            cfg.voice.cloud.model = profile.model.clone();
        }
    }
}

/// 依据界面下拉索引同步 VAD 端点检测模型类型与模型路径
fn update_vad_model_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    let vad_models = waylan_voice::list_vad_models();
    let vad_idx = window.get_vad_model_index() as usize;
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
    let ns_idx = window.get_ns_model_index() as usize;
    if let Some(m) = ns_filtered.get(ns_idx) {
        cfg.voice.afe.ns_engine = match m.path.as_deref() {
            Some("__deepfilternet__") => NsEngineType::DeepFilterNet,
            Some("__gtcrn__") => NsEngineType::Gtcrn,
            _ => NsEngineType::Rnnoise,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::mask_api_key;

    #[test]
    fn test_mask_api_key_is_char_boundary_safe() {
        assert_eq!(mask_api_key("sk-1234567890"), "sk-1******");
        assert_eq!(mask_api_key("密钥密钥密钥密"), "密钥密钥******");
        assert_eq!(mask_api_key("short"), "******");
        assert_eq!(mask_api_key(""), "******");
    }
}
