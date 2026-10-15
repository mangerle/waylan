//! 语音识别 (ASR) 引擎抽象特型与多后端实现（离线模型/云端API/仿真测试）
//!
//! # 设计原理
//! - **实现初衷**：定义统一解耦的 `VoiceRecognizer` Trait，支持用户在本地端侧离线模型（Paraformer/Zipformer）、
//!   线上云端 API 与仿真测试之间自由热插拔与平滑切换。
//! - **核心优势**：本地离线模型文件路径完全外部化配置，支持随时更换不同尺寸与架构的权重；云端 API 支持无模型轻量部署。
//! - **代价与局限**：离线模型依赖用户提前部署 ONNX 权重文件，未就绪时自动给出友好的中文定位提示并优雅降级。

use std::path::PathBuf;

use log::{info, warn};
use sherpa_onnx::{
    OfflinePunctuation, OfflinePunctuationConfig, OnlineRecognizer, OnlineRecognizerConfig,
    OnlineStream,
};
use waylan_core::{
    CloudAsrConfig, OfflineAsrConfig, OfflineModelType, VoiceConfig, VoiceEngineType,
};

use crate::error::VoiceError;

/// 语音识别引擎核心特型
pub trait VoiceRecognizer: Send + Sync {
    /// 输入一段 16kHz 单声道 PCM 音频采样，返回当前识别到的增量/流式候选文字（如有）
    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError>;

    /// 结束当前说话流并生成最终确认的整句文字
    fn finish(&mut self) -> Result<String, VoiceError>;

    /// 取消或重置当前识别引擎状态
    fn reset(&mut self);
}

/// 仿真测试语音识别器 (Mock Voice Recognizer)
#[derive(Debug, Default)]
pub struct MockVoiceRecognizer {
    /// 累计接收到的采样总数（16000 采样 = 1 秒）
    accumulated_samples: usize,
    /// 当前流式文本输出阶段索引
    step_index: usize,
    /// 预设的演示句子列表
    demo_sentences: Vec<&'static str>,
}

impl MockVoiceRecognizer {
    /// 创建仿真语音识别器
    pub fn new() -> Self {
        Self {
            accumulated_samples: 0,
            step_index: 0,
            demo_sentences: vec!["微澜", "微澜语音", "微澜语音输入", "微澜语音输入测试成功"],
        }
    }
}

impl VoiceRecognizer for MockVoiceRecognizer {
    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError> {
        self.accumulated_samples += samples.len();

        let current_step = self.accumulated_samples / 4800;
        if current_step > self.step_index && !self.demo_sentences.is_empty() {
            self.step_index = current_step;
            let idx = (self.step_index.saturating_sub(1)).min(self.demo_sentences.len() - 1);
            let text = self.demo_sentences[idx].to_string();
            return Ok(Some(text));
        }

        Ok(None)
    }

    fn finish(&mut self) -> Result<String, VoiceError> {
        let final_text = if self.step_index > 0 && !self.demo_sentences.is_empty() {
            let idx = (self.step_index.saturating_sub(1)).min(self.demo_sentences.len() - 1);
            self.demo_sentences[idx].to_string()
        } else {
            "微澜语音输入".to_string()
        };

        self.reset();
        Ok(final_text)
    }

    fn reset(&mut self) {
        self.accumulated_samples = 0;
        self.step_index = 0;
    }
}

/// 解析指定的本地 ASR 模型目录路径（支持自定义配置、XDG 用户目录、系统共享目录与本地相对目录）
pub fn resolve_asr_model_dir(
    custom_dir: Option<&str>,
    model_type: OfflineModelType,
) -> Option<PathBuf> {
    if let Some(dir_str) = custom_dir {
        let p = PathBuf::from(dir_str);
        if p.exists() {
            return Some(p);
        }
    }
    let subname = match model_type {
        OfflineModelType::Paraformer => {
            "asr/sherpa-onnx-streaming-paraformer-bilingual-zh-en-2023-03-03"
        }
        OfflineModelType::Zipformer => {
            "asr/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20"
        }
        OfflineModelType::Whisper => "asr/whisper-tiny-zh",
    };
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        let p = home_path.join(".local/share/waylan/models").join(subname);
        if p.exists() {
            return Some(p);
        }
        let p_short = home_path.join(".local/share/waylan/models/asr/paraformer-zh");
        if p_short.exists() {
            return Some(p_short);
        }
    }
    let sys_p = PathBuf::from("/usr/share/waylan/models").join(subname);
    if sys_p.exists() {
        return Some(sys_p);
    }
    let sys_p_short = PathBuf::from("/usr/share/waylan/models/asr/paraformer-zh");
    if sys_p_short.exists() {
        return Some(sys_p_short);
    }
    let local_p = PathBuf::from("models").join(subname);
    if local_p.exists() {
        return Some(local_p);
    }
    let local_p_short = PathBuf::from("models/asr/paraformer-zh");
    if local_p_short.exists() {
        return Some(local_p_short);
    }
    None
}

/// sherpa-onnx 流式识别会话运行时
struct SherpaOnlineSession {
    recognizer: OnlineRecognizer,
    stream: OnlineStream,
    last_text: String,
}

/// 本地端侧离线语音识别器（基于 sherpa-onnx 原生支持 Paraformer / Zipformer / Whisper 权重）
pub struct OfflineVoiceRecognizer {
    /// 配置参数
    config: OfflineAsrConfig,
    /// 模型所在根目录解析后的绝对路径
    resolved_model_dir: Option<PathBuf>,
    /// 是否检测到完整的模型权重文件集合
    models_ready: bool,
    /// 活跃的 sherpa-onnx 流式识别会话
    active_session: Option<SherpaOnlineSession>,
    /// 仿真降级识别器（当模型尚未下载或配置错误时优雅保底）
    fallback_mock: MockVoiceRecognizer,
}

impl OfflineVoiceRecognizer {
    /// 创建本地离线识别器并校验模型权重完整性
    pub fn new(config: OfflineAsrConfig) -> Self {
        let resolved_dir = resolve_asr_model_dir(config.model_dir.as_deref(), config.model_type);
        let mut models_ready = false;
        let mut active_session = None;

        if let Some(ref path) = resolved_dir {
            let enc_ok = path.join(&config.encoder).exists();
            let dec_ok = path.join(&config.decoder).exists();
            let tok_ok = path.join(&config.tokens).exists();

            if enc_ok && dec_ok && tok_ok {
                info!(
                    "[本地 ASR] 检测到完整本地离线模型文件 (架构: {:?}, 目录: {:?})",
                    config.model_type, path
                );
                models_ready = true;

                let mut rec_cfg = OnlineRecognizerConfig::default();
                rec_cfg.feat_config.sample_rate = 16000;
                rec_cfg.feat_config.feature_dim = 80;
                rec_cfg.decoding_method = Some("greedy_search".to_string());
                rec_cfg.model_config.num_threads = 2;
                rec_cfg.model_config.provider = Some("cpu".to_string());
                rec_cfg.model_config.tokens =
                    Some(path.join(&config.tokens).to_string_lossy().to_string());

                match config.model_type {
                    OfflineModelType::Paraformer => {
                        rec_cfg.model_config.paraformer.encoder =
                            Some(path.join(&config.encoder).to_string_lossy().to_string());
                        rec_cfg.model_config.paraformer.decoder =
                            Some(path.join(&config.decoder).to_string_lossy().to_string());
                    }
                    OfflineModelType::Zipformer => {
                        rec_cfg.model_config.transducer.encoder =
                            Some(path.join(&config.encoder).to_string_lossy().to_string());
                        rec_cfg.model_config.transducer.decoder =
                            Some(path.join(&config.decoder).to_string_lossy().to_string());
                        for candidate in &["joiner.int8.onnx", "joiner.onnx"] {
                            let jp = path.join(candidate);
                            if jp.exists() {
                                rec_cfg.model_config.transducer.joiner =
                                    Some(jp.to_string_lossy().to_string());
                                break;
                            }
                        }
                    }
                    OfflineModelType::Whisper => {
                        warn!("[本地 ASR] Whisper 暂未配置在线流式架构，请配置流式模型");
                    }
                }

                if let Some(recognizer) = OnlineRecognizer::create(&rec_cfg) {
                    info!("[本地 ASR] 成功创建 sherpa-onnx 原生流式识别引擎");
                    let stream = recognizer.create_stream();
                    active_session = Some(SherpaOnlineSession {
                        recognizer,
                        stream,
                        last_text: String::new(),
                    });
                } else {
                    warn!("[本地 ASR] 创建 sherpa-onnx 识别器失败，正在使用仿真器优雅降级");
                }
            } else {
                warn!(
                    "[本地 ASR] 模型目录缺少必要权重文件 (encoder: {}, decoder: {}, tokens: {})",
                    enc_ok, dec_ok, tok_ok
                );
            }
        } else {
            info!("[本地 ASR] 未检测到已下载的本地 ASR 模型，使用仿真识别器保底");
        }

        Self {
            config,
            resolved_model_dir: resolved_dir,
            models_ready,
            active_session,
            fallback_mock: MockVoiceRecognizer::new(),
        }
    }

    /// 模型文件是否已就绪
    pub fn is_models_ready(&self) -> bool {
        self.models_ready
    }

    /// 获取解析后的模型所在目录路径
    pub fn resolved_model_dir(&self) -> Option<&PathBuf> {
        self.resolved_model_dir.as_ref()
    }

    /// 获取当前加载的模型架构类型
    pub fn model_type(&self) -> OfflineModelType {
        self.config.model_type
    }
}

impl VoiceRecognizer for OfflineVoiceRecognizer {
    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError> {
        if let Some(session) = &mut self.active_session {
            session.stream.accept_waveform(16000, samples);
            while session.recognizer.is_ready(&session.stream) {
                session.recognizer.decode(&session.stream);
            }
            if let Some(res) = session.recognizer.get_result(&session.stream) {
                if !res.text.is_empty() && res.text != session.last_text {
                    session.last_text = res.text.clone();
                    return Ok(Some(res.text));
                }
            }
            return Ok(None);
        }
        self.fallback_mock.feed_pcm(samples)
    }

    fn finish(&mut self) -> Result<String, VoiceError> {
        if let Some(session) = &mut self.active_session {
            session.stream.input_finished();
            while session.recognizer.is_ready(&session.stream) {
                session.recognizer.decode(&session.stream);
            }
            let final_text = if let Some(res) = session.recognizer.get_result(&session.stream) {
                if !res.text.is_empty() {
                    res.text
                } else {
                    session.last_text.clone()
                }
            } else {
                session.last_text.clone()
            };
            session.recognizer.reset(&session.stream);
            session.last_text.clear();
            return Ok(final_text);
        }
        self.fallback_mock.finish()
    }

    fn reset(&mut self) {
        if let Some(session) = &mut self.active_session {
            session.recognizer.reset(&session.stream);
            session.last_text.clear();
        }
        self.fallback_mock.reset();
    }
}

/// 将 16kHz f32 PCM 浮点音频采样序列编码为标准 16 位单声道 WAV 字节数据
pub fn encode_pcm_to_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
    let block_align = num_channels * (bits_per_sample / 8);
    let subchunk2_size = (samples.len() * 2) as u32;
    let chunk_size = 36 + subchunk2_size;

    let mut wav = Vec::with_capacity(44 + subchunk2_size as usize);
    // RIFF 块描述
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&chunk_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // "fmt " 子块
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // PCM 格式块固定大小 16
    wav.extend_from_slice(&1u16.to_le_bytes()); // 格式 1 = 线性 PCM
    wav.extend_from_slice(&num_channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());

    // "data" 数据子块
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&subchunk2_size.to_le_bytes());

    // 转换 f32 (-1.0 ~ 1.0) 为 i16 小端 PCM 字节
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let val = (clamped * 32767.0).round() as i16;
        wav.extend_from_slice(&val.to_le_bytes());
    }

    wav
}

/// 线上云端语音识别器（支持自由切换不同云服务与 API Key）
pub struct CloudVoiceRecognizer {
    /// 云端配置
    config: CloudAsrConfig,
    /// 累积待发送的 PCM 音频缓冲（16kHz f32）
    pcm_buffer: Vec<f32>,
}

impl CloudVoiceRecognizer {
    /// 创建云端识别器
    pub fn new(config: CloudAsrConfig) -> Self {
        info!(
            "[云端 ASR] 正在初始化线上语音服务客户端 (协议: {:?}, 地址: {})",
            config.provider, config.endpoint
        );
        Self {
            config,
            pcm_buffer: Vec::with_capacity(16_000 * 5),
        }
    }

    /// 获取当前云端配置
    pub fn config(&self) -> &CloudAsrConfig {
        &self.config
    }
}

impl VoiceRecognizer for CloudVoiceRecognizer {
    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError> {
        self.pcm_buffer.extend_from_slice(samples);
        // 云端批处理/分块上传接口在说话过程中累积采样
        Ok(None)
    }

    fn finish(&mut self) -> Result<String, VoiceError> {
        if self.pcm_buffer.is_empty() {
            return Ok(String::new());
        }

        let api_key = match &self.config.api_key {
            Some(key) if !key.trim().is_empty() => key.trim().to_string(),
            _ => {
                warn!("[云端 ASR] 未配置有效的 API Key 凭据，无法发起远端识别");
                self.reset();
                return Ok("未配置云端 API Key".to_string());
            }
        };

        let wav_data = encode_pcm_to_wav(&self.pcm_buffer, 16_000);
        let duration_secs = self.pcm_buffer.len() as f64 / 16000.0;
        self.reset();

        let endpoint = if self.config.endpoint.trim().is_empty() {
            "https://api.openai.com/v1/audio/transcriptions".to_string()
        } else {
            self.config.endpoint.clone()
        };

        info!(
            "[云端 ASR] 正在向远端服务发起识别请求 (地址: {}, 音频: {:.2}s, 字节数: {})",
            endpoint,
            duration_secs,
            wav_data.len()
        );

        // 构造标准的 multipart/form-data 报文体
        let boundary = format!(
            "----WayLanBoundary{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let mut body = Vec::new();

        // 1. model 字段
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n\r\n");
        let model_name = if self.config.model.trim().is_empty() {
            "whisper-1"
        } else {
            self.config.model.trim()
        };
        body.extend_from_slice(model_name.as_bytes());
        body.extend_from_slice(b"\r\n");

        // 2. language 字段
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"language\"\r\n\r\n");
        body.extend_from_slice(b"zh\r\n");

        // 3. file 音频文件内容
        body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\n",
        );
        body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
        body.extend_from_slice(&wav_data);
        body.extend_from_slice(b"\r\n");

        // 结束边界标记
        body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

        let timeout = std::time::Duration::from_secs(15);
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();

        let resp_result = agent
            .post(&endpoint)
            .set("Authorization", &format!("Bearer {}", api_key))
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={}", boundary),
            )
            .send_bytes(&body);

        match resp_result {
            Ok(resp) => {
                let status = resp.status();
                if (200..300).contains(&status) {
                    let resp_str =
                        resp.into_string()
                            .map_err(|e| VoiceError::RecognizerFailed {
                                engine_type: "云端 Whisper".to_string(),
                                message: format!("解析云端响应字符串失败: {}", e),
                            })?;
                    // 尝试从标准 Whisper 返回 JSON 中解析识别文本
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&resp_str) {
                        if let Some(text) = val.get("text").and_then(|t| t.as_str()) {
                            info!("[云端 ASR] 识别成功，文本长度: {}", text.len());
                            return Ok(text.to_string());
                        }
                    }
                    Ok(resp_str)
                } else {
                    let err_msg = format!("云端服务器返回错误状态码: {}", status);
                    warn!("[云端 ASR] {}", err_msg);
                    Ok(err_msg)
                }
            }
            Err(e) => {
                let err_msg = format!("请求云端语音识别服务失败: {}", e);
                warn!("[云端 ASR] {}", err_msg);
                Ok(err_msg)
            }
        }
    }

    fn reset(&mut self) {
        self.pcm_buffer.clear();
    }
}

/// 离线标点符号预测还原器（基于 sherpa-onnx CT-Transformer 标点模型）
pub struct PunctuationRestorer {
    model: Option<OfflinePunctuation>,
}

impl PunctuationRestorer {
    /// 创建标点符号还原器，自动探测本地模型文件
    pub fn new() -> Self {
        Self::with_custom_path(None)
    }

    /// 创建标点符号还原器，优先使用指定的自定义模型路径，未指定时自动探测系统与用户默认模型
    pub fn with_custom_path(custom_path: Option<&str>) -> Self {
        let model = Self::try_create_model(custom_path);
        Self { model }
    }

    /// 尝试定位并加载标点模型
    fn try_create_model(custom_path: Option<&str>) -> Option<OfflinePunctuation> {
        let model_path = if let Some(p) = custom_path.filter(|s| !s.trim().is_empty()) {
            let pb = PathBuf::from(p);
            if pb.is_file() {
                Some(pb)
            } else {
                Self::resolve_punct_model_path()
            }
        } else {
            Self::resolve_punct_model_path()
        }?;
        info!("[标点还原] 检测到本地标点模型文件: {:?}", model_path);
        let mut config = OfflinePunctuationConfig::default();
        config.model.ct_transformer = Some(model_path.to_string_lossy().to_string());
        config.model.num_threads = 1;
        config.model.provider = Some("cpu".to_string());

        match OfflinePunctuation::create(&config) {
            Some(p) => {
                info!("[标点还原] 成功创建 sherpa-onnx 离线标点符号预测模型");
                Some(p)
            }
            None => {
                warn!("[标点还原] 初始化标点模型失败，将回退到基础标点规则");
                None
            }
        }
    }

    /// 定位本地标点模型权重文件
    fn resolve_punct_model_path() -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            candidates
                .push(PathBuf::from(&home).join(".local/share/waylan/models/punct/model.onnx"));
            candidates.push(PathBuf::from(&home).join(".local/share/waylan/models/punct/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12/model.onnx"));
        }
        candidates.push(PathBuf::from("/usr/share/waylan/models/punct/model.onnx"));
        candidates.push(PathBuf::from("/usr/share/waylan/models/punct/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12/model.onnx"));
        candidates.push(PathBuf::from("models/punct/model.onnx"));
        candidates.into_iter().find(|p| p.is_file())
    }

    /// 为文本预测并添加标点符号；若无可用模型，执行基础句末补全
    pub fn restore(&self, text: &str) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }

        if let Some(ref punct) = self.model {
            if let Some(res) = punct.add_punctuation(trimmed) {
                let clean = res.trim();
                if !clean.is_empty() {
                    return clean.to_string();
                }
            }
        }

        // 基础兜底规则：末尾无标点时补充中文句号
        let mut fallback = trimmed.to_string();
        if !fallback.ends_with('。')
            && !fallback.ends_with('，')
            && !fallback.ends_with('！')
            && !fallback.ends_with('？')
            && !fallback.ends_with('；')
        {
            fallback.push('。');
        }
        fallback
    }
}

impl Default for PunctuationRestorer {
    fn default() -> Self {
        Self::new()
    }
}

/// 识别引擎工厂函数：根据系统配置动态构造对应的识别器后端
pub fn create_recognizer(config: &VoiceConfig) -> Box<dyn VoiceRecognizer> {
    match config.engine_type {
        VoiceEngineType::Offline => Box::new(OfflineVoiceRecognizer::new(config.offline.clone())),
        VoiceEngineType::Cloud => Box::new(CloudVoiceRecognizer::new(config.cloud.clone())),
        VoiceEngineType::Mock => Box::new(MockVoiceRecognizer::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::{CloudProviderType, VoiceEngineType};

    #[test]
    fn test_mock_recognizer_flow() {
        let mut rec = MockVoiceRecognizer::new();
        let chunk = vec![0.1; 5000];
        let partial = rec.feed_pcm(&chunk).unwrap();
        assert!(partial.is_some());
        assert_eq!(partial.unwrap(), "微澜");

        let finish_res = rec.finish().unwrap();
        assert_eq!(finish_res, "微澜");
    }

    #[test]
    fn test_recognizer_factory_offline() {
        let cfg = VoiceConfig {
            engine_type: VoiceEngineType::Offline,
            ..Default::default()
        };
        let mut rec = create_recognizer(&cfg);
        let chunk = vec![0.0; 5000];
        let feed_res = rec.feed_pcm(&chunk);
        assert!(feed_res.is_ok());
        let finish_res = rec.finish();
        assert!(finish_res.is_ok());
    }

    #[test]
    fn test_offline_recognizer_detects_models() {
        let cfg = OfflineAsrConfig::default();
        let rec = OfflineVoiceRecognizer::new(cfg);
        // 验证部署在标准 XDG 路径的模型是否成功被自探测发现并加载
        assert!(rec.is_models_ready());
    }

    #[test]
    fn test_recognizer_factory_cloud_unconfigured() {
        let mut cfg = VoiceConfig {
            engine_type: VoiceEngineType::Cloud,
            ..Default::default()
        };
        cfg.cloud.provider = CloudProviderType::OpenAiCompatible;
        cfg.cloud.api_key = None;

        let mut rec = create_recognizer(&cfg);
        let chunk = vec![0.1; 16000];
        assert!(rec.feed_pcm(&chunk).unwrap().is_none());
        let res = rec.finish().unwrap();
        assert_eq!(res, "未配置云端 API Key");
    }

    #[test]
    fn test_encode_pcm_to_wav_header() {
        let samples = vec![0.0f32; 16000];
        let wav = encode_pcm_to_wav(&samples, 16000);
        assert!(wav.len() >= 44 + 32000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn test_punctuation_restorer_flow() {
        let restorer = PunctuationRestorer::new();
        let result = restorer.restore("你好世界");
        // 无论是否有模型权重，都应当输出带有标点的中文自然句子
        assert!(
            result.ends_with('。')
                || result.ends_with('！')
                || result.ends_with('？')
                || result.contains('，'),
            "标点预测应生成规范标点: {}",
            result
        );
    }
}
