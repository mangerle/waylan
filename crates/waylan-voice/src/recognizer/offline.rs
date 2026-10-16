//! 本地端侧离线语音识别器（基于 sherpa-onnx 原生支持 Paraformer / Zipformer 权重）

use std::path::PathBuf;

use log::{info, warn};
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};
use waylan_core::{OfflineAsrConfig, OfflineModelType};

use super::traits::{MockVoiceRecognizer, VoiceRecognizer};
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
            // 检查单文件 CTC 模型（如 2025 年的 Zipformer2-Small-CTC model.int8.onnx）
            let ctc_model_file = ["model.int8.onnx", "model.onnx"]
                .iter()
                .map(|f| path.join(f))
                .find(|p| p.exists());

            let enc_file = if path.join(&config.encoder).exists() {
                Some(path.join(&config.encoder))
            } else if path.join("encoder.int8.onnx").exists() {
                Some(path.join("encoder.int8.onnx"))
            } else if path.join("encoder.onnx").exists() {
                Some(path.join("encoder.onnx"))
            } else {
                None
            };

            let dec_file = if path.join(&config.decoder).exists() {
                Some(path.join(&config.decoder))
            } else if path.join("decoder.onnx").exists() {
                Some(path.join("decoder.onnx"))
            } else if path.join("decoder.int8.onnx").exists() {
                Some(path.join("decoder.int8.onnx"))
            } else {
                None
            };

            let tok_file = if path.join(&config.tokens).exists() {
                Some(path.join(&config.tokens))
            } else if path.join("tokens.txt").exists() {
                Some(path.join("tokens.txt"))
            } else {
                None
            };

            let joiner_file = ["joiner.int8.onnx", "joiner.onnx"]
                .iter()
                .map(|f| path.join(f))
                .find(|p| p.exists());

            // 自动侦测实际 Zipformer 流式模型架构：
            // 1. 若存在单个 ctc 模型文件（如 model.int8.onnx），判定为 Zipformer2-CTC 极轻量流式引擎
            // 2. 若存在 joiner 权重，判定为 Zipformer-Transducer 多文件流式引擎
            let is_ctc = ctc_model_file.is_some() || joiner_file.is_none();

            let files_ready = if is_ctc {
                ctc_model_file.is_some() && tok_file.is_some()
            } else {
                enc_file.is_some()
                    && dec_file.is_some()
                    && tok_file.is_some()
                    && joiner_file.is_some()
            };

            if files_ready {
                info!(
                    "[本地 ASR] 检测到完整本地离线 Zipformer 模型 (是否CTC: {}, 目录: {:?})",
                    is_ctc, path
                );
                models_ready = true;

                let mut rec_cfg = OnlineRecognizerConfig::default();
                rec_cfg.feat_config.sample_rate = 16000;
                rec_cfg.feat_config.feature_dim = 80;
                rec_cfg.decoding_method = Some("greedy_search".to_string());
                rec_cfg.model_config.num_threads = 2;
                rec_cfg.model_config.provider = Some("cpu".to_string());
                rec_cfg.model_config.tokens = tok_file.map(|p| p.to_string_lossy().to_string());

                if is_ctc {
                    rec_cfg.model_config.zipformer2_ctc.model =
                        ctc_model_file.map(|p| p.to_string_lossy().to_string());
                } else {
                    rec_cfg.model_config.transducer.encoder =
                        enc_file.map(|p| p.to_string_lossy().to_string());
                    rec_cfg.model_config.transducer.decoder =
                        dec_file.map(|p| p.to_string_lossy().to_string());
                    rec_cfg.model_config.transducer.joiner =
                        joiner_file.map(|p| p.to_string_lossy().to_string());
                    rec_cfg.model_config.model_type = Some("zipformer2".to_string());
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
                    "[本地 ASR] 模型目录缺少必要权重文件 (encoder: {}, decoder: {}, tokens: {}, joiner: {})",
                    enc_file.is_some(),
                    dec_file.is_some(),
                    tok_file.is_some(),
                    joiner_file.is_some()
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
