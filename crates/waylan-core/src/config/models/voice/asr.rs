//! 语音识别引擎（本地离线 ASR 与云端 Whisper 兼容服务）配置模型

use serde::{Deserialize, Serialize};

/// 语音识别引擎类型选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VoiceEngineType {
    /// 本地离线端侧推理（Paraformer/Zipformer 等）
    #[default]
    Offline,
    /// 线上云端 API 识别（OpenAI/阿里/腾讯等）
    Cloud,
    /// 仿真测试（零外部依赖测试链路）
    Mock,
}

/// 本地离线 ASR 识别模型架构类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OfflineModelType {
    /// 阿里 SenseVoice-Small（中英多语言、自带标点、超轻量极速，默认推荐）
    #[default]
    #[serde(
        alias = "sensevoice",
        alias = "SenseVoice",
        alias = "sense_voice",
        alias = "SenseVoiceSmall",
        alias = "Zipformer",
        alias = "zipformer",
        alias = "Paraformer",
        alias = "paraformer"
    )]
    SenseVoice,
    /// 阿里 FunASR-Nano（多方言/多语言、自带标点、高准确率）
    #[serde(
        alias = "funasr_nano",
        alias = "FunAsrNano",
        alias = "fun_asr_nano",
        alias = "FunASRNano"
    )]
    FunAsrNano,
    /// 阿里通义 Qwen3-ASR（多模态语言模型架构、强大上下文理解与语义纠错、自带标点）
    #[serde(
        alias = "qwen3_asr",
        alias = "qwen3-asr",
        alias = "Qwen3Asr",
        alias = "Qwen3ASR",
        alias = "qwen3"
    )]
    Qwen3Asr,
    /// 小红书 FireRedASR2（中文普通话与多方言识别顶尖准确率）
    #[serde(
        alias = "fireredasr2",
        alias = "firered_asr2",
        alias = "FireRedAsr2",
        alias = "FireRedASR2",
        alias = "fire-red-asr2",
        alias = "firered"
    )]
    FireRedAsr2,
}

impl OfflineModelType {
    /// 当前模型是否原生自带标点符号（SenseVoice、FunAsrNano 与 Qwen3Asr 原生自带）
    #[inline]
    pub fn has_native_punctuation(&self) -> bool {
        match self {
            Self::SenseVoice | Self::FunAsrNano | Self::Qwen3Asr => true,
            Self::FireRedAsr2 => false,
        }
    }

    /// 获取模型标识名称
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SenseVoice => "SenseVoice-Small",
            Self::FunAsrNano => "Fun-ASR-Nano",
            Self::Qwen3Asr => "Qwen3-ASR",
            Self::FireRedAsr2 => "FireRedASR2",
        }
    }
}

/// 本地离线 ASR 模型配置（支持自由更换模型文件与架构）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OfflineAsrConfig {
    /// 模型架构类型
    pub model_type: OfflineModelType,
    /// 模型文件所在根目录路径（例如 "~/.local/share/waylan/models/asr/sensevoice-small"）
    pub model_dir: Option<String>,
    /// 语言选项（默认 "auto"，支持 "zh", "en", "yue", "ja", "ko" 等）
    pub language: String,
    /// CPU 推理工作线程数
    pub num_threads: usize,
}

impl Default for OfflineAsrConfig {
    fn default() -> Self {
        Self {
            model_type: OfflineModelType::default(),
            model_dir: None,
            language: "auto".to_string(),
            num_threads: 2,
        }
    }
}

/// 云端线上 ASR 服务提供商协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CloudProviderType {
    /// OpenAI 兼容音频转录接口（Whisper API）
    #[default]
    OpenAiCompatible,
    /// 阿里云流式语音识别
    Aliyun,
    /// 腾讯云流式语音识别
    Tencent,
    /// 自定义 WebSocket 流式服务端
    CustomWebSocket,
}

/// 线上云端语音识别配置（支持自由配置云端 API 与凭证）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudAsrConfig {
    /// 云端提供商协议
    pub provider: CloudProviderType,
    /// 线上服务接口地址（URL）
    pub endpoint: String,
    /// 鉴权 API Key / Token（敏感机密凭证）
    pub api_key: Option<String>,
    /// 指定的远端模型名称
    pub model: String,
}

impl CloudAsrConfig {
    /// 规整云端服务终端 URL 与模型名称的空白及默认值兜底
    pub(crate) fn sanitize_endpoint_and_model(endpoint: &mut String, model: &mut String) {
        let trimmed_ep = endpoint.trim();
        if trimmed_ep.is_empty() {
            *endpoint = "https://api.openai.com/v1/audio/transcriptions".to_string();
        } else if trimmed_ep.len() != endpoint.len() {
            *endpoint = trimmed_ep.to_string();
        }

        let trimmed_model = model.trim();
        if trimmed_model.is_empty() {
            *model = "whisper-1".to_string();
        } else if trimmed_model.len() != model.len() {
            *model = trimmed_model.to_string();
        }
    }

    /// 规整云端语音识别配置字段边界
    pub fn sanitize(&mut self) {
        Self::sanitize_endpoint_and_model(&mut self.endpoint, &mut self.model);
    }
}

impl Default for CloudAsrConfig {
    fn default() -> Self {
        Self {
            provider: CloudProviderType::default(),
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            api_key: None,
            model: "whisper-1".to_string(),
        }
    }
}

/// 线上云端语音识别服务配置实例（Profile）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudProfile {
    /// 唯一标识 ID（例如 "openai"、"groq"、"fastwhisper"）
    pub id: String,
    /// 界面友好展示备注名称（例如 "OpenAI 官方"、"Groq 极速转写"）
    pub name: String,
    /// 云端服务协议类型
    pub provider: CloudProviderType,
    /// 线上服务接口地址（URL）
    pub endpoint: String,
    /// 鉴权 API Key / Token（敏感机密凭据）
    pub api_key: Option<String>,
    /// 指定的远端模型名称（例如 "whisper-1"）
    pub model: String,
}

impl Default for CloudProfile {
    fn default() -> Self {
        let base = CloudAsrConfig::default();
        Self {
            id: "default-openai".to_string(),
            name: "OpenAI 官方".to_string(),
            provider: base.provider,
            endpoint: base.endpoint,
            api_key: base.api_key,
            model: base.model,
        }
    }
}

impl From<&CloudProfile> for CloudAsrConfig {
    fn from(profile: &CloudProfile) -> Self {
        Self {
            provider: profile.provider,
            endpoint: profile.endpoint.clone(),
            api_key: profile.api_key.clone(),
            model: profile.model.clone(),
        }
    }
}
