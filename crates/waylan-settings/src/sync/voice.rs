use crate::{CloudProfileItem, SettingsWindow};
use waylan_core::config::AppConfig;
use waylan_voice::{NsEngineType, VadModelType, VoiceEngineType, VoiceVisualEffect};

/// 同步语音输入配置与模型状态至 Slint 界面属性
pub(super) fn sync_voice_to_window(window: &SettingsWindow, cfg: &AppConfig) {
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
    window.set_voice_visual_effect_index(match cfg.voice.interaction.visual_effect {
        VoiceVisualEffect::DynamicWaveform => 0,
        VoiceVisualEffect::BreathingPulse => 1,
        VoiceVisualEffect::ClassicText => 2,
    });

    // 云端服务 Profile 列表装载
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
                    .map(|k| {
                        if k.len() > 6 {
                            format!("{}******", &k[..4])
                        } else {
                            "******".to_string()
                        }
                    })
                    .unwrap_or_else(|| "未设置".to_string()),
            ),
            model: slint::SharedString::from(&p.model),
            is_active: active_cloud_id == Some(p.id.as_str()),
        })
        .collect();
    window.set_cloud_profiles(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(cloud_items),
    )));

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
    "当前处于【本地离线模式】: 采用阿里 Paraformer 流式 int8 ONNX 引擎，所有音频计算均在本地完成，零延迟、零网络依赖、保障隐私安全。".to_string()
}

/// 解析当前配置中生效的 ASR 模型列表下标、是否云端以及状态提示文本
fn resolve_asr_selection(
    cfg: &AppConfig,
    local_asrs: &[waylan_voice::ModelItem],
) -> (usize, bool, String) {
    if let Some(ref sel) = cfg.voice.selected_asr {
        if let Some(cloud_id) = sel.strip_prefix("cloud:") {
            if let Some(pos) = cfg
                .voice
                .cloud_profiles
                .iter()
                .position(|p| p.id == cloud_id)
            {
                let profile = &cfg.voice.cloud_profiles[pos];
                let notice = format!(
                    "当前处于【云端转写模式 - {}】: 说话结束后将音频打包发送至目标云端 API，由远端 {} 模型转录。支持超大参数高精转写，请保持网络畅通。",
                    profile.name, profile.model
                );
                return (local_asrs.len() + pos, true, notice);
            }
        } else if let Some(ref dir) = cfg.voice.offline.model_dir {
            if let Some(pos) = find_model_index_by_path(local_asrs, dir) {
                return (pos, false, default_offline_notice());
            }
        }
    } else if let Some(ref dir) = cfg.voice.offline.model_dir {
        if let Some(pos) = find_model_index_by_path(local_asrs, dir) {
            return (pos, false, default_offline_notice());
        }
    }

    (0, false, default_offline_notice())
}

/// 同步 ASR 模型下拉列表（本地离线模型 + 线上云端 Profile）
fn sync_asr_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    let local_asrs = waylan_voice::list_asr_models();
    let mut all_asr_names: Vec<slint::SharedString> = Vec::new();
    for m in &local_asrs {
        all_asr_names.push(slint::SharedString::from(format!(
            "[本地] {}",
            m.display_name
        )));
    }
    for p in &cfg.voice.cloud_profiles {
        all_asr_names.push(slint::SharedString::from(format!(
            "[云端] {} ({})",
            p.name, p.model
        )));
    }

    let (selected_idx, is_cloud, notice) = resolve_asr_selection(cfg, &local_asrs);
    window.set_asr_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(all_asr_names),
    )));
    window.set_asr_model_index(selected_idx as i32);
    window.set_asr_notice_text(slint::SharedString::from(notice));
    window.set_is_asr_cloud(is_cloud);
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

/// 同步标点符号预测模型下拉列表与选中索引
fn sync_punct_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    let punct_models = waylan_voice::list_punct_models();
    let punct_names: Vec<slint::SharedString> = punct_models
        .iter()
        .map(|m| slint::SharedString::from(&m.display_name))
        .collect();
    let punct_idx = cfg
        .voice
        .punct_model_path
        .as_deref()
        .and_then(|path| find_model_index_by_path(&punct_models, path))
        .unwrap_or(0);
    window.set_punct_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(punct_names),
    )));
    window.set_punct_model_index(punct_idx as i32);
}

/// 同步人声智能降噪 (NS) 模型下拉列表与选中索引
fn sync_ns_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    let ns_models = waylan_voice::list_ns_models();
    let ns_names: Vec<slint::SharedString> = ns_models
        .iter()
        .filter(|m| m.path.as_deref() != Some("__bypass__"))
        .map(|m| slint::SharedString::from(&m.display_name))
        .collect();
    let ns_idx = match cfg.voice.afe.ns_engine {
        NsEngineType::Rnnoise => 0,
        NsEngineType::DeepFilterNet => 1,
        NsEngineType::Gtcrn => 2,
        _ => 0,
    };
    window.set_ns_model_names(slint::ModelRc::from(std::rc::Rc::new(
        slint::VecModel::from(ns_names),
    )));
    window.set_ns_model_index(ns_idx);
}

/// 同步语音四大模型（ASR / VAD / 标点 / NS降噪）列表与当前选中项至界面
fn sync_voice_models_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    sync_asr_models_to_window(window, cfg);
    sync_vad_models_to_window(window, cfg);
    sync_punct_models_to_window(window, cfg);
    sync_ns_models_to_window(window, cfg);
}

/// 从 Slint 界面属性增量读取语音输入配置
pub(super) fn update_voice_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    cfg.voice.enabled = window.get_voice_enabled();

    // 连续听写开关快捷键 (Toggle)
    let toggle_str = window.get_toggle_shortcut().trim().to_string();
    cfg.voice.toggle_shortcut = if toggle_str.is_empty()
        || toggle_str.eq_ignore_ascii_case("disabled")
        || toggle_str == "已禁用"
    {
        "已禁用".to_string()
    } else {
        toggle_str
    };

    // 长按对讲机快捷键 (Push-to-Talk)
    let ptt_str = window.get_push_to_talk_shortcut().trim().to_string();
    cfg.voice.push_to_talk_shortcut =
        if ptt_str.is_empty() || ptt_str.eq_ignore_ascii_case("disabled") || ptt_str == "已禁用"
        {
            "已禁用".to_string()
        } else {
            ptt_str
        };

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
    cfg.voice.interaction.visual_effect = match window.get_voice_visual_effect_index() {
        1 => VoiceVisualEffect::BreathingPulse,
        2 => VoiceVisualEffect::ClassicText,
        _ => VoiceVisualEffect::DynamicWaveform,
    };

    update_voice_models_from_window(window, cfg);
}

/// 从 Slint 界面属性读取模型选择并持久化至配置
fn update_voice_models_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    let local_asrs = waylan_voice::list_asr_models();
    let asr_idx = window.get_asr_model_index() as usize;

    if asr_idx < local_asrs.len() {
        // 选中本地离线 ASR 模型
        cfg.voice.engine_type = VoiceEngineType::Offline;
        cfg.voice.selected_asr = None;
        if let Some(m) = local_asrs.get(asr_idx) {
            cfg.voice.offline.model_dir = m.path.clone();
        }
    } else {
        // 选中云端 Profile
        let cloud_idx = asr_idx - local_asrs.len();
        if let Some(profile) = cfg.voice.cloud_profiles.get(cloud_idx) {
            cfg.voice.engine_type = VoiceEngineType::Cloud;
            cfg.voice.selected_asr = Some(format!("cloud:{}", profile.id));
            cfg.voice.cloud.endpoint = profile.endpoint.clone();
            cfg.voice.cloud.api_key = profile.api_key.clone();
            cfg.voice.cloud.model = profile.model.clone();
        }
    }

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

    let punct_models = waylan_voice::list_punct_models();
    let punct_idx = window.get_punct_model_index() as usize;
    if let Some(m) = punct_models.get(punct_idx) {
        cfg.voice.punct_model_path = m.path.clone();
    }

    let ns_models = waylan_voice::list_ns_models();
    let ns_filtered: Vec<_> = ns_models
        .into_iter()
        .filter(|m| m.path.as_deref() != Some("__bypass__"))
        .collect();
    let ns_idx = window.get_ns_model_index() as usize;
    if let Some(m) = ns_filtered.get(ns_idx) {
        if m.path.as_deref() == Some("__rnnoise__") {
            cfg.voice.afe.ns_engine = NsEngineType::Rnnoise;
            cfg.voice.afe.custom_ns_model_path = None;
        } else if m.path.as_deref() == Some("__deepfilternet__") {
            cfg.voice.afe.ns_engine = NsEngineType::DeepFilterNet;
            cfg.voice.afe.custom_ns_model_path = None;
        } else if m.path.as_deref() == Some("__gtcrn__") {
            cfg.voice.afe.ns_engine = NsEngineType::Gtcrn;
            cfg.voice.afe.custom_ns_model_path = None;
        }
    }
}
