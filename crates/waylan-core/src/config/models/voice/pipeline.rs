//! 语音音频处理流水线（VAD 端点检测、AFE 前端降噪增强与 ITN 文本规整）配置模型

use serde::{Deserialize, Serialize};

/// VAD 端点检测算法/模型类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VadModelType {
    /// Silero 神经网络 VAD 模型（基于 ONNX 权重文件）
    #[default]
    SileroOnnx,
    /// 轻量能量门限（零模型纯数学算法）
    Energy,
}

/// VAD 端点检测配置（支持自由更换模型与算法）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    /// VAD 类型
    pub vad_type: VadModelType,
    /// 自定义 VAD 模型文件路径（例如 "~/.local/share/waylan/models/vad/silero_vad.onnx"）
    pub model_path: Option<String>,
    /// 人声置信度门限（0.0 ~ 1.0）
    pub speech_threshold: f32,
    /// 静音超时判定时长（毫秒，默认 800ms）
    pub silence_timeout_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            vad_type: VadModelType::default(),
            model_path: None,
            speech_threshold: 0.5,
            silence_timeout_ms: 800,
        }
    }
}

/// 智能降噪引擎类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NsEngineType {
    /// 内置极轻量 RNNoise (纯 Rust 静态集成，零外部依赖)
    #[default]
    Rnnoise,
    /// 深度滤波 DeepFilterNet (超强消除机械键盘敲击声)
    DeepFilterNet,
    /// 新一代 GTCRN 深度神经网络降噪 (523KB SOTA 极轻量)
    Gtcrn,
    /// 直通禁用降噪
    Bypass,
}

/// 音频前端预处理与降噪配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFrontEndConfig {
    /// 是否开启前端音频增强流水线总控（为 false 时全部旁路直通 Bypass）
    pub enabled: bool,
    /// 是否开启 80Hz 高通滤波（切除机械低频底噪与电磁交流声）
    pub high_pass_filter: bool,
    /// 是否开启人声智能降噪
    pub noise_suppression: bool,
    /// 智能降噪引擎模式（轻量 RNNoise 或深度 DeepFilterNet）
    pub ns_engine: NsEngineType,
    /// 是否开启数字自动增益控制 (AGC)
    pub auto_gain_control: bool,
}

impl Default for AudioFrontEndConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            high_pass_filter: true,
            noise_suppression: true,
            ns_engine: NsEngineType::default(),
            auto_gain_control: true,
        }
    }
}

/// 阶段五：文本规整与语气词过滤 (ITN) 配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceItnConfig {
    /// 是否开启大写数字自动转阿拉伯数字（如“三点五”转“3.5”）
    pub convert_numbers: bool,
    /// 是否开启口语停顿语气词与结巴自动过滤（过滤“呃、啊、这个、那个”等口癖）
    pub filter_filler_words: bool,
    /// 是否开启中英文自动盘古空格
    pub pangu_spacing: bool,
}

impl Default for VoiceItnConfig {
    fn default() -> Self {
        Self {
            convert_numbers: true,
            filter_filler_words: true,
            pangu_spacing: true,
        }
    }
}
