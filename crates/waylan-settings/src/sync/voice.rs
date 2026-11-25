//! 语音通用开关、快捷键与流式交互配置同步

use super::voice_models::{sync_voice_models_to_window, update_voice_models_from_window};
use crate::{CloudProfileItem, SettingsWindow};
use waylan_core::config::{AppConfig, UiIndexEnum};
use waylan_voice::VoiceVisualEffect;

/// VAD 静音超时档位选项表（索引对应下拉选项，值为毫秒）
const VAD_SILENCE_MS_TABLE: [u64; 4] = [400, 600, 800, 1000];

/// 将静音超时毫秒值映射至下拉挡位索引
fn vad_silence_timeout_to_index(timeout_ms: u64) -> i32 {
    match timeout_ms {
        0..=500 => 0,
        501..=700 => 1,
        701..=900 => 2,
        _ => 3,
    }
}

/// 将下拉挡位索引还原为静音超时毫秒值
fn vad_silence_index_to_timeout(index: i32) -> u64 {
    usize::try_from(index)
        .ok()
        .and_then(|idx| VAD_SILENCE_MS_TABLE.get(idx).copied())
        .unwrap_or(600)
}

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

    let toggle_str = cfg.voice.toggle_shortcut.trim();
    window.set_toggle_shortcut(slint::SharedString::from(toggle_str));

    let ptt_str = cfg.voice.push_to_talk_shortcut.trim();
    window.set_push_to_talk_shortcut(slint::SharedString::from(ptt_str));

    // 阶段一 AFE 前端增强
    window.set_afe_enabled(cfg.voice.afe.enabled);
    window.set_high_pass_filter(cfg.voice.afe.high_pass_filter);
    window.set_noise_suppression(cfg.voice.afe.noise_suppression);
    window.set_auto_gain_control(cfg.voice.afe.auto_gain_control);

    // 阶段二 VAD 端点截断
    window.set_vad_silence_delay_index(vad_silence_timeout_to_index(
        cfg.voice.vad.silence_timeout_ms,
    ));

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
    let active_cloud_id = cfg
        .voice
        .selected_asr
        .as_deref()
        .and_then(|sel| sel.strip_prefix("cloud:"));
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

/// 从 Slint 界面属性增量读取语音输入配置
pub(super) fn update_voice_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    cfg.voice.enabled = window.get_voice_enabled();
    cfg.voice.toggle_shortcut =
        waylan_core::config::normalize_shortcut_text(&window.get_toggle_shortcut());
    cfg.voice.push_to_talk_shortcut =
        waylan_core::config::normalize_shortcut_text(&window.get_push_to_talk_shortcut());

    // 阶段一 AFE 前端增强
    cfg.voice.afe.enabled = window.get_afe_enabled();
    cfg.voice.afe.high_pass_filter = window.get_high_pass_filter();
    cfg.voice.afe.noise_suppression = window.get_noise_suppression();
    cfg.voice.afe.auto_gain_control = window.get_auto_gain_control();

    // 阶段二 VAD 端点检测截断时间
    cfg.voice.vad.silence_timeout_ms =
        vad_silence_index_to_timeout(window.get_vad_silence_delay_index());

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
