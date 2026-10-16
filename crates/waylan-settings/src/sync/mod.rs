//! 配置模型与 Slint 窗口属性双向数据同步模块
//!
//! # 设计原理
//! - **实现初衷**：解耦界面渲染与核心配置领域模型，集中管理 GUI 属性与持久化配置的映射。
//! - **核心优势**：
//!   1. 集中防守，对索引越界、异常数值进行安全边界校验与降级；
//!   2. 避免在事件回调中分散编写冗长脆弱的属性映射逻辑；
//!   3. 支持无缝双向转换，便于单元测试与自动化验证。

mod general;
mod voice;

use general::{sync_general_to_window, update_general_from_window};
use voice::{sync_voice_to_window, update_voice_from_window};

use crate::SettingsWindow;
use waylan_core::config::AppConfig;

/// 将 AppConfig 数据应用到 Slint 界面属性
pub fn sync_config_to_window(window: &SettingsWindow, cfg: &AppConfig) {
    sync_general_to_window(window, cfg);
    sync_voice_to_window(window, cfg);
}

/// 基于 Slint 界面属性增量更新已有的 AppConfig（保留未在界面中展示的高级配置项，防误清空）
pub fn update_config_from_window(window: &SettingsWindow, cfg: &mut AppConfig) {
    update_general_from_window(window, cfg);
    update_voice_from_window(window, cfg);
    cfg.sanitize();
}

/// 从 Slint 界面属性读取并构造 AppConfig 数据
pub fn extract_config_from_window(window: &SettingsWindow) -> AppConfig {
    let mut cfg = AppConfig::load_or_create();
    update_config_from_window(window, &mut cfg);
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::config::{
        CandidateDensity, CandidateIndexStyle, CandidateThemePreset, CandidateWindowMode,
        FullWidthCharShortcut, FullWidthPunctShortcut, TraditionalShortcut, VoiceVisualEffect,
    };

    #[test]
    fn test_sync_and_update_preserves_custom_icons() {
        let window = SettingsWindow::new().expect("创建窗口对象应成功");
        let initial_cfg = AppConfig {
            tray_icon_theme_path: Some("/opt/my-theme".to_string()),
            tray_icon_zh: Some("custom-zh.svg".to_string()),
            tray_icon_en: Some("custom-en.svg".to_string()),
            enable_tray: Some(true),
            exclusive_input_source: true,
            enable_u_mode: false,
            enable_i_mode: false,
            filter_uncommon_charset: true,
            show_pinyin_comment: false,
            auto_dedup: false,
            ..Default::default()
        };

        sync_config_to_window(&window, &initial_cfg);

        // 验证界面读取与同步状态（Some(true) + true 对应索引 0: 微澜专属彩色托盘）
        assert_eq!(window.get_enable_tray_index(), 0);
        assert!(!window.get_enable_u_mode());
        assert!(!window.get_enable_i_mode());
        assert!(window.get_filter_uncommon_charset());
        assert!(!window.get_show_pinyin_comment());
        assert!(!window.get_auto_dedup());

        assert_eq!(window.get_traditional_shortcut_index(), 0);
        assert_eq!(window.get_full_width_char_shortcut_index(), 0);
        assert_eq!(window.get_full_width_punct_shortcut_index(), 0);
        assert_eq!(window.get_window_mode_index(), 0); // 默认 WaylanCustom
        assert_eq!(window.get_density_index(), 1); // 默认 Normal
        assert_eq!(window.get_theme_preset_index(), 0); // 默认 Emerald
        assert_eq!(window.get_candidate_item_spacing(), 8.0);
        assert_eq!(window.get_candidate_padding_v(), 4.0);
        assert_eq!(window.get_candidate_padding_h(), 8.0);
        assert_eq!(window.get_candidate_corner_radius(), 8.0);
        assert_eq!(window.get_candidate_opacity(), 95.0);
        assert_eq!(window.get_voice_visual_effect_index(), 0); // 默认 DynamicWaveform

        // 模拟用户在界面修改某项（切为索引 1: GNOME 原生单图标，索引 2: 圆圈序号，修改快捷键，切为原生候选框模式）
        assert_eq!(window.get_index_style_index(), 0);
        window.set_page_size_val(8.0);
        window.set_enable_tray_index(1);
        window.set_index_style_index(2);
        window.set_window_mode_index(1); // Native 模式
        window.set_density_index(0); // Compact 紧凑
        window.set_theme_preset_index(2); // IceBlue
        window.set_candidate_item_spacing(12.0);
        window.set_candidate_padding_v(6.0);
        window.set_candidate_padding_h(14.0);
        window.set_candidate_corner_radius(10.0);
        window.set_candidate_opacity(90.0);
        window.set_voice_visual_effect_index(1); // BreathingPulse
        window.set_traditional_shortcut_index(1); // CtrlAltF
        window.set_full_width_char_shortcut_index(1); // CtrlShiftSpace
        window.set_full_width_punct_shortcut_index(1); // AltPeriod

        let mut updated_cfg = initial_cfg.clone();
        update_config_from_window(&window, &mut updated_cfg);

        // 验证修改生效
        assert_eq!(updated_cfg.page_size, 8);
        assert_eq!(
            updated_cfg.candidate_window_mode,
            CandidateWindowMode::Native
        );
        assert_eq!(updated_cfg.candidate_density, CandidateDensity::Compact);
        assert_eq!(updated_cfg.candidate_theme, CandidateThemePreset::IceBlue);
        assert_eq!(updated_cfg.candidate_item_spacing, 12.0);
        assert_eq!(updated_cfg.candidate_padding_v, 6.0);
        assert_eq!(updated_cfg.candidate_padding_h, 14.0);
        assert_eq!(updated_cfg.candidate_corner_radius, 10.0);
        assert_eq!(updated_cfg.candidate_opacity, 90);
        assert_eq!(
            updated_cfg.voice.interaction.visual_effect,
            VoiceVisualEffect::BreathingPulse
        );
        assert_eq!(
            updated_cfg.candidate_index_style,
            CandidateIndexStyle::Circled
        );
        assert_eq!(updated_cfg.enable_tray, Some(false));
        assert!(!updated_cfg.exclusive_input_source);
        assert_eq!(
            updated_cfg.traditional_shortcut,
            TraditionalShortcut::CtrlAltF
        );
        assert_eq!(
            updated_cfg.full_width_char_shortcut,
            FullWidthCharShortcut::CtrlShiftSpace
        );
        assert_eq!(
            updated_cfg.full_width_punct_shortcut,
            FullWidthPunctShortcut::AltPeriod
        );
        // 关键：未在界面展示的高级图标配置绝对不可被清空
        assert_eq!(
            updated_cfg.tray_icon_theme_path.as_deref(),
            Some("/opt/my-theme")
        );
        assert_eq!(updated_cfg.tray_icon_zh.as_deref(), Some("custom-zh.svg"));
        assert_eq!(updated_cfg.tray_icon_en.as_deref(), Some("custom-en.svg"));

        // 验证语音双独立快捷键与默认开关状态
        assert!(!window.get_voice_enabled());
        assert_eq!(window.get_toggle_shortcut().as_str(), "Ctrl + Alt + V");
        assert_eq!(window.get_toggle_shortcut_index(), 0);
        assert_eq!(window.get_push_to_talk_shortcut().as_str(), "F8");
        assert_eq!(window.get_ptt_shortcut_index(), 0);

        // 模拟用户在 UI 界面自定义设置：开启语音输入，切换长按对讲机为 Super + V，禁用连续听写
        window.set_voice_enabled(true);
        window.set_ptt_shortcut_index(3);
        window.set_push_to_talk_shortcut("Super + V".into());
        window.set_toggle_shortcut_index(5);
        window.set_toggle_shortcut("已禁用".into());

        update_config_from_window(&window, &mut updated_cfg);

        // 验证语音快捷键修改并持久化成功
        assert!(updated_cfg.voice.enabled);
        assert_eq!(updated_cfg.voice.push_to_talk_shortcut, "Super + V");
        assert_eq!(updated_cfg.voice.toggle_shortcut, "已禁用");
        assert!(updated_cfg.voice.get_effective_ptt_shortcut().is_some());
        assert!(updated_cfg.voice.get_effective_toggle_shortcut().is_none());

        // 验证语音流水线与云端 Profile 双向同步与模型切换
        let mut voice_pipeline_cfg = updated_cfg.clone();
        voice_pipeline_cfg.voice.afe.enabled = true;
        voice_pipeline_cfg.voice.afe.high_pass_filter = false;
        voice_pipeline_cfg.voice.afe.noise_suppression = true;
        voice_pipeline_cfg.voice.afe.auto_gain_control = false;
        voice_pipeline_cfg.voice.vad.silence_timeout_ms = 800;
        voice_pipeline_cfg.voice.auto_punctuation = false;
        voice_pipeline_cfg.voice.cloud_profiles = vec![waylan_core::CloudProfile {
            id: "test-openai".to_string(),
            name: "测试 OpenAI".to_string(),
            provider: waylan_core::CloudProviderType::OpenAiCompatible,
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            api_key: Some("sk-test12345678".to_string()),
            model: "whisper-1".to_string(),
        }];
        voice_pipeline_cfg.voice.selected_asr = Some("cloud:test-openai".to_string());
        voice_pipeline_cfg.voice.engine_type = waylan_core::VoiceEngineType::Cloud;

        sync_config_to_window(&window, &voice_pipeline_cfg);

        // 验证初始同步到窗口属性
        assert!(window.get_afe_enabled());
        assert!(!window.get_high_pass_filter());
        assert!(window.get_noise_suppression());
        assert!(!window.get_auto_gain_control());
        assert_eq!(window.get_vad_silence_delay_index(), 2); // 800ms 对应索引 2
        assert!(!window.get_auto_punctuation());
        assert!(window.get_is_asr_cloud());
        assert!(window.get_itn_convert_numbers());
        assert!(window.get_itn_filter_filler_words());
        assert!(window.get_itn_pangu_spacing());
        assert!(window.get_voice_real_time_preview());
        assert!(window.get_voice_show_floating_bar());

        // 模拟用户在流水线界面修改：开启自动增益、切换 400ms 停顿、开启标点、切回本地 ASR (索引 0)、关闭语气词过滤、关闭波形条
        window.set_auto_gain_control(true);
        window.set_vad_silence_delay_index(0); // 400ms
        window.set_auto_punctuation(true);
        window.set_asr_model_index(0); // 本地模型
        window.set_itn_filter_filler_words(false);
        window.set_voice_show_floating_bar(false);

        let mut roundtrip_cfg = voice_pipeline_cfg.clone();
        update_config_from_window(&window, &mut roundtrip_cfg);

        // 验证回写持久化配置
        assert!(roundtrip_cfg.voice.afe.auto_gain_control);
        assert_eq!(roundtrip_cfg.voice.vad.silence_timeout_ms, 400);
        assert!(roundtrip_cfg.voice.auto_punctuation);
        assert!(!roundtrip_cfg.voice.itn.filter_filler_words);
        assert!(roundtrip_cfg.voice.itn.convert_numbers);
        assert!(roundtrip_cfg.voice.itn.pangu_spacing);
        assert!(roundtrip_cfg.voice.interaction.real_time_preview);
        assert!(!roundtrip_cfg.voice.interaction.show_floating_bar);
        assert_eq!(
            roundtrip_cfg.voice.engine_type,
            waylan_core::VoiceEngineType::Offline
        );
        assert!(roundtrip_cfg.voice.selected_asr.is_none());
        assert_eq!(roundtrip_cfg.voice.cloud_profiles.len(), 1);
        assert_eq!(roundtrip_cfg.voice.cloud_profiles[0].name, "测试 OpenAI");
    }
}
