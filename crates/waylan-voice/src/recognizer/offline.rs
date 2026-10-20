//! 本地端侧离线语音识别器（基于 sherpa-onnx 原生支持 Paraformer / Zipformer 权重）

use std::path::PathBuf;

use log::{info, warn};
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};
use waylan_core::{OfflineAsrConfig, OfflineModelType};

use super::traits::VoiceRecognizer;
use crate::error::VoiceError;

/// 解析指定的本地 ASR 模型目录路径（支持自定义配置、XDG 用户目录、系统共享目录与本地相对目录）
pub fn resolve_asr_model_dir(
    custom_dir: Option<&str>,
    _model_type: OfflineModelType,
) -> Option<PathBuf> {
    if let Some(dir_str) = custom_dir {
        let p = PathBuf::from(dir_str);
        if p.exists() {
            return Some(p);
        }
    }
    let subnames = [
        "asr/zipformer-zh",
        "asr/sherpa-onnx-streaming-zipformer-small-ctc-zh-int8-2025-04-01",
        "asr/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30",
        "asr/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20",
    ];

    let mut search_roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        search_roots.push(PathBuf::from(home).join(".local/share/waylan/models"));
    }
    search_roots.push(PathBuf::from("/usr/share/waylan/models"));
    search_roots.push(PathBuf::from("models"));
    search_roots.push(PathBuf::from("../../models"));
    search_roots.push(PathBuf::from("../models"));

    for root in &search_roots {
        for sub in &subnames {
            let p = root.join(sub);
            if p.exists() {
                return Some(p);
            }
        }
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
}

/// 侦测到的本地 Zipformer 模型文件集合
enum DetectedZipformerModel {
    /// 单文件 CTC 极轻量流式模型
    Ctc { model: PathBuf, tokens: PathBuf },
    /// Transducer 多文件流式模型（encoder, decoder, joiner, tokens）
    Transducer {
        encoder: PathBuf,
        decoder: PathBuf,
        joiner: PathBuf,
        tokens: PathBuf,
    },
}

/// 优先检查自定义配置路径是否存在，若不存在则在默认候选文件名中匹配
fn resolve_candidate_file(
    dir: &std::path::Path,
    configured: &str,
    defaults: &[&str],
) -> Option<PathBuf> {
    let conf_path = dir.join(configured);
    if conf_path.exists() {
        return Some(conf_path);
    }
    defaults.iter().map(|f| dir.join(f)).find(|p| p.exists())
}

/// 在指定目录中尝试探测 CTC 或 Transducer 架构的完整模型文件
fn detect_zipformer_files(
    path: &std::path::Path,
    config: &OfflineAsrConfig,
) -> Option<DetectedZipformerModel> {
    let ctc_model = ["model.int8.onnx", "model.onnx"]
        .iter()
        .map(|f| path.join(f))
        .find(|p| p.exists());

    let tokens = resolve_candidate_file(path, &config.tokens, &["tokens.txt"]);
    let joiner = ["joiner.int8.onnx", "joiner.onnx"]
        .iter()
        .map(|f| path.join(f))
        .find(|p| p.exists());

    // 优先匹配单文件 CTC 模型
    if let (Some(model), Some(tokens)) = (ctc_model, tokens.clone()) {
        if joiner.is_none() {
            return Some(DetectedZipformerModel::Ctc { model, tokens });
        }
    }

    let encoder = resolve_candidate_file(
        path,
        &config.encoder,
        &["encoder.int8.onnx", "encoder.onnx"],
    );
    let decoder = resolve_candidate_file(
        path,
        &config.decoder,
        &["decoder.int8.onnx", "decoder.onnx"],
    );

    if let (Some(encoder), Some(decoder), Some(joiner), Some(tokens)) =
        (encoder, decoder, joiner, tokens)
    {
        Some(DetectedZipformerModel::Transducer {
            encoder,
            decoder,
            joiner,
            tokens,
        })
    } else {
        None
    }
}

/// 根据探测到的模型文件配置并创建 sherpa-onnx 在线流式会话
fn create_online_session(model: &DetectedZipformerModel) -> Option<SherpaOnlineSession> {
    let mut rec_cfg = OnlineRecognizerConfig::default();
    rec_cfg.feat_config.sample_rate = 16000;
    rec_cfg.feat_config.feature_dim = 80;
    rec_cfg.decoding_method = Some("greedy_search".to_string());
    rec_cfg.model_config.num_threads = 2;
    rec_cfg.model_config.provider = Some("cpu".to_string());

    match model {
        DetectedZipformerModel::Ctc { model, tokens } => {
            rec_cfg.model_config.tokens = Some(tokens.to_string_lossy().to_string());
            rec_cfg.model_config.zipformer2_ctc.model = Some(model.to_string_lossy().to_string());
        }
        DetectedZipformerModel::Transducer {
            encoder,
            decoder,
            joiner,
            tokens,
        } => {
            rec_cfg.model_config.tokens = Some(tokens.to_string_lossy().to_string());
            rec_cfg.model_config.transducer.encoder = Some(encoder.to_string_lossy().to_string());
            rec_cfg.model_config.transducer.decoder = Some(decoder.to_string_lossy().to_string());
            rec_cfg.model_config.transducer.joiner = Some(joiner.to_string_lossy().to_string());
            rec_cfg.model_config.model_type = Some("zipformer2".to_string());
        }
    }

    let recognizer = OnlineRecognizer::create(&rec_cfg)?;
    info!("[本地 ASR] 成功创建 sherpa-onnx 原生流式识别引擎");
    let stream = recognizer.create_stream();
    Some(SherpaOnlineSession {
        recognizer,
        stream,
        last_text: String::new(),
    })
}

impl OfflineVoiceRecognizer {
    /// 创建本地离线识别器并校验模型权重完整性
    pub fn new(config: OfflineAsrConfig) -> Self {
        let resolved_dir = resolve_asr_model_dir(config.model_dir.as_deref(), config.model_type);
        let mut models_ready = false;
        let mut active_session = None;

        if let Some(ref path) = resolved_dir {
            if let Some(detected) = detect_zipformer_files(path, &config) {
                let is_ctc = matches!(detected, DetectedZipformerModel::Ctc { .. });
                info!(
                    "[本地 ASR] 检测到完整本地离线 Zipformer 模型 (是否CTC: {}, 目录: {:?})",
                    is_ctc, path
                );
                models_ready = true;
                if let Some(session) = create_online_session(&detected) {
                    active_session = Some(session);
                } else {
                    warn!(
                        "[本地 ASR] 模型文件已找到，但 sherpa-onnx 识别器创建失败；本次语音输入将明确报错而非伪造文本"
                    );
                }
            } else {
                warn!("[本地 ASR] 模型目录 {:?} 缺少必要权重文件", path);
            }
        } else {
            info!("[本地 ASR] 未检测到已下载的本地 ASR 模型，语音输入将返回明确错误");
        }

        Self {
            config,
            resolved_model_dir: resolved_dir,
            models_ready,
            active_session,
        }
    }

    /// 生成模型不可用错误
    fn unavailable_error(&self) -> VoiceError {
        let reason = match self.resolved_model_dir {
            Some(ref path) => format!(
                "模型目录 {} 缺少必要的 encoder/decoder/tokens 权重，或识别器初始化失败",
                path.display()
            ),
            None => "未找到本地离线 ASR 模型，请先在设置中心导入模型".to_string(),
        };
        VoiceError::RecognizerUnavailable { reason }
    }

    /// 模型文件是否已就绪
    pub fn is_models_ready(&self) -> bool {
        self.models_ready
    }

    /// 构造一个确定缺少模型的识别器实例（仅用于验证失败关闭契约）
    #[cfg(test)]
    pub(crate) fn unavailable_for_test() -> Self {
        Self {
            config: OfflineAsrConfig::default(),
            resolved_model_dir: None,
            models_ready: false,
            active_session: None,
        }
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
    fn is_ready(&self) -> bool {
        self.active_session.is_some()
    }

    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError> {
        let Some(session) = &mut self.active_session else {
            return Err(self.unavailable_error());
        };
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
        Ok(None)
    }

    fn finish(&mut self) -> Result<String, VoiceError> {
        let Some(session) = &mut self.active_session else {
            return Err(self.unavailable_error());
        };
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
        Ok(final_text)
    }

    fn reset(&mut self) {
        if let Some(session) = &mut self.active_session {
            session.recognizer.reset(&session.stream);
            session.last_text.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_model_fails_closed_without_fake_text() {
        let mut recognizer = OfflineVoiceRecognizer::unavailable_for_test();
        assert!(!recognizer.is_ready());
        assert!(matches!(
            recognizer.feed_pcm(&[0.0; 160]),
            Err(VoiceError::RecognizerUnavailable { .. })
        ));
        assert!(matches!(
            recognizer.finish(),
            Err(VoiceError::RecognizerUnavailable { .. })
        ));
    }
}
