//! 语音输入界面交互展现与浮层动效配置模型

use serde::{Deserialize, Serialize};

/// 语音输入浮层视觉动效风格
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VoiceVisualEffect {
    /// 现代多频律动声浪胶囊（彩色声柱平滑起伏，默认推荐）
    #[default]
    DynamicWaveform,
    /// 流光呼吸光环微卡片（微澜青绿呼吸式脉冲微光）
    BreathingPulse,
    /// 经典文本提示（平滑回退至传统的“正在聆听中... ▏▎▍”文字形态）
    ClassicText,
}

/// 阶段六：交互展现与流式上屏配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceInteractionConfig {
    /// 是否在光标处实时文字预览（边说边出字；关闭后为松手/停顿后整句上屏）
    pub real_time_preview: bool,
    /// 是否显示录音浮动波形条
    pub show_floating_bar: bool,
    /// 语音浮动提示视觉风格
    pub visual_effect: VoiceVisualEffect,
}

impl Default for VoiceInteractionConfig {
    fn default() -> Self {
        Self {
            real_time_preview: true,
            show_floating_bar: true,
            visual_effect: VoiceVisualEffect::default(),
        }
    }
}
