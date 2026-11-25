//! 语音输入功能配置模型与数据契约
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法可选的语音识别扩展提供统一的序列化数据结构（TOML 映射）。
//! - **核心优势**：纯数据载体，零音频硬件/模型推理/网络 IO 依赖；按 ASR、流水线与交互三个子模块裂变，
//!   所有音频采集、端点检测及模型推理实现完全下沉至 `waylan-voice`。
//! - **代价与局限**：模型具体有效性校验需委托由 `waylan-voice` 运行时执行。

mod asr;
mod interaction;
mod pipeline;

pub use asr::{
    CloudAsrConfig, CloudProfile, CloudProviderType, OfflineAsrConfig, OfflineModelType,
    VoiceEngineType,
};
pub use interaction::{VoiceInteractionConfig, VoiceVisualEffect};
pub use pipeline::{AudioFrontEndConfig, NsEngineType, VadConfig, VadModelType, VoiceItnConfig};

use super::shortcuts::ParsedShortcut;
use serde::{Deserialize, Serialize};

/// 语音输入配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// 是否开启语音输入功能
    pub enabled: bool,
    /// 连续听写开关快捷键（按一下开启，VAD分句上屏，按回车或再次按快捷键结束）
    pub toggle_shortcut: String,
    /// 长按对讲机快捷键（按住说话，松手即上屏）
    pub push_to_talk_shortcut: String,
    /// 识别引擎工作模式（本地离线 / 线上云端 / 仿真测试）
    pub engine_type: VoiceEngineType,
    /// 当前统一选中的 ASR 识别目标（为 None 或以 "local:" 开头时走本地；以 "cloud:<id>" 开头时走对应云端服务）
    pub selected_asr: Option<String>,
    /// 是否自动在识别文本末尾补全句号
    pub auto_punctuation: bool,
    /// 1. VAD 端点检测配置（支持自由更换模型与算法）
    pub vad: VadConfig,
    /// 2. 本地离线识别配置（支持自由更换模型与路径）
    pub offline: OfflineAsrConfig,
    /// 3. 线上云端识别配置（保留旧版本单一兼容配置）
    pub cloud: CloudAsrConfig,
    /// 3.1 线上云端多服务配置列表（支持配置多个独立的 Whisper 兼容服务）
    pub cloud_profiles: Vec<CloudProfile>,
    /// 4. 音频前端预处理与降噪配置 (HPF + NS + AGC)
    pub afe: AudioFrontEndConfig,
    /// 5. 文本规整与语气词过滤配置 (ITN)
    pub itn: VoiceItnConfig,
    /// 6. 交互展现与流式上屏配置
    pub interaction: VoiceInteractionConfig,
    /// 麦克风音频输入设备名称（为 None 时自动使用系统默认设备）
    pub device_name: Option<String>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            toggle_shortcut: "Ctrl + Alt + V".to_string(),
            push_to_talk_shortcut: "F8".to_string(),
            engine_type: VoiceEngineType::default(),
            selected_asr: None,
            auto_punctuation: true,
            vad: VadConfig::default(),
            offline: OfflineAsrConfig::default(),
            cloud: CloudAsrConfig::default(),
            cloud_profiles: vec![CloudProfile::default()],
            afe: AudioFrontEndConfig::default(),
            itn: VoiceItnConfig::default(),
            interaction: VoiceInteractionConfig::default(),
            device_name: None,
        }
    }
}

impl VoiceConfig {
    /// 校验并钳制语音子配置数值边界，防止非法线程数、NaN 阈值或空白终端地址传入运行时
    pub fn sanitize(&mut self) {
        self.vad.silence_timeout_ms = self.vad.silence_timeout_ms.clamp(200, 5000);
        self.vad.speech_threshold = if self.vad.speech_threshold.is_finite() {
            self.vad.speech_threshold.clamp(0.01, 0.99)
        } else {
            0.5
        };
        self.offline.num_threads = self.offline.num_threads.clamp(1, 32);
        self.cloud.sanitize();
        for profile in &mut self.cloud_profiles {
            CloudAsrConfig::sanitize_endpoint_and_model(&mut profile.endpoint, &mut profile.model);
        }
    }

    /// 解析当前激活的云端 Profile 标识（当 `selected_asr` 为 `"cloud:<id>"` 时返回 `<id>`）
    pub fn active_cloud_profile_id(&self) -> Option<&str> {
        self.selected_asr
            .as_deref()
            .and_then(|sel| sel.strip_prefix("cloud:"))
    }

    /// 统一解析当前生效的云端识别配置（收敛 `selected_asr`、`cloud_profiles` 与旧版 `cloud` 的优先级）
    pub fn resolve_active_cloud_config(&self) -> Option<CloudAsrConfig> {
        if let Some(profile_id) = self.active_cloud_profile_id() {
            return Some(
                self.cloud_profiles
                    .iter()
                    .find(|p| p.id == profile_id)
                    .map(CloudAsrConfig::from)
                    .unwrap_or_else(|| self.cloud.clone()),
            );
        }
        if self.engine_type == VoiceEngineType::Cloud {
            Some(self.cloud.clone())
        } else {
            None
        }
    }

    /// 获取当前生效的连续听写开关快捷键 (Toggle)
    pub fn get_effective_toggle_shortcut(&self) -> Option<ParsedShortcut> {
        if !self.enabled {
            return None;
        }
        ParsedShortcut::parse(&self.toggle_shortcut)
    }

    /// 获取当前生效的长按对讲机快捷键 (Push-to-Talk)
    pub fn get_effective_ptt_shortcut(&self) -> Option<ParsedShortcut> {
        if !self.enabled {
            return None;
        }
        ParsedShortcut::parse(&self.push_to_talk_shortcut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::models::KeyModifiers;

    #[test]
    fn test_voice_config_effective_dual_shortcuts() {
        let mut cfg = VoiceConfig::default();
        // 默认状态下语音为关闭状态，所有快捷键均不应生效
        assert!(cfg.get_effective_toggle_shortcut().is_none());
        assert!(cfg.get_effective_ptt_shortcut().is_none());

        // 显式开启语音输入功能
        cfg.enabled = true;
        let toggle = cfg
            .get_effective_toggle_shortcut()
            .expect("开启后默认开关按键必须生效");
        assert!(toggle.matches(47, &KeyModifiers::new(true, true, false, false)));

        let ptt = cfg
            .get_effective_ptt_shortcut()
            .expect("开启后默认对讲机按键必须生效");
        assert!(ptt.matches(66, &KeyModifiers::default()));

        cfg.toggle_shortcut = "已禁用".to_string();
        assert!(cfg.get_effective_toggle_shortcut().is_none());
        assert!(cfg.get_effective_ptt_shortcut().is_some());

        cfg.enabled = false;
        assert!(cfg.get_effective_ptt_shortcut().is_none());
    }

    #[test]
    fn test_audio_front_end_config_defaults_and_serde() {
        let default_afe = AudioFrontEndConfig::default();
        assert!(default_afe.high_pass_filter);
        assert!(default_afe.noise_suppression);
        assert_eq!(default_afe.ns_engine, NsEngineType::Rnnoise);
        assert!(default_afe.auto_gain_control);

        // 验证空 JSON 反序列化可自动使用全量默认值
        let json_empty = "{}";
        let parsed: AudioFrontEndConfig =
            serde_json::from_str(json_empty).expect("空 JSON 必须成功使用默认值反序列化");
        assert_eq!(parsed, default_afe);

        // 验证序列化与恢复
        let serialized = serde_json::to_string(&default_afe).expect("必须成功序列化");
        let restored: AudioFrontEndConfig =
            serde_json::from_str(&serialized).expect("必须成功从序列化还原");
        assert_eq!(restored, default_afe);
    }
}
