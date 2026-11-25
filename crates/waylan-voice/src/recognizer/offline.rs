//! 本地端侧离线语音识别器（基于 sherpa-onnx 原生支持 SenseVoice-Small / Fun-ASR-Nano 权重）

use std::path::{Path, PathBuf};

use log::{info, warn};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig};
use waylan_core::{OfflineAsrConfig, OfflineModelType};

use super::traits::VoiceRecognizer;
use crate::error::VoiceError;

/// 解析指定的本地 ASR 模型目录路径（支持自定义配置、XDG 用户目录、系统共享目录与本地相对目录）
pub fn resolve_asr_model_dir(
    custom_dir: Option<&str>,
    model_type: OfflineModelType,
) -> Option<PathBuf> {
    let subnames: &[&str] = match model_type {
        OfflineModelType::SenseVoice => &[
            "asr/sensevoice-small",
            "asr/sensevoice",
            "asr/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17-int8",
            "asr/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
            "asr/SenseVoice-Small",
        ],
        OfflineModelType::FunAsrNano => &[
            "asr/funasr-nano",
            "asr/fun_asr_nano",
            "asr/sherpa-onnx-funasr-nano-int8",
            "asr/sherpa-onnx-funasr-nano",
            "asr/Fun-ASR-Nano",
        ],
        OfflineModelType::Qwen3Asr => &[
            "asr/qwen3-asr",
            "asr/qwen3-asr-0.6b",
            "asr/qwen3-asr-1.7b",
            "asr/sherpa-onnx-qwen3-asr-0.6B-int8",
            "asr/sherpa-onnx-qwen3-asr-1.7B-int8",
            "asr/Qwen3-ASR",
        ],
        OfflineModelType::FireRedAsr2 => &[
            "asr/fireredasr2",
            "asr/firered-asr2",
            "asr/fire-red-asr2",
            "asr/sherpa-onnx-fire-red-asr2-ctc-zh_en-int8",
            "asr/sherpa-onnx-fire-red-asr2-ctc-zh_en",
            "asr/FireRedASR2",
        ],
    };

    let custom = custom_dir
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Path::new);
    waylan_core::config::resolve_model_path(custom, subnames)
}

/// 侦测到的本地离线模型文件集合（收敛为 SenseVoice、Fun-ASR-Nano、Qwen3-ASR 与 FireRedASR2 结构）
enum DetectedOfflineModel {
    /// 阿里 SenseVoice-Small 模型（单模型 + 词表）
    SenseVoice { model: PathBuf, tokens: PathBuf },
    /// 阿里 FunASR-Nano 模型（多组件 ONNX + Tokenizer）
    FunAsrNano {
        encoder_adaptor: PathBuf,
        llm: PathBuf,
        embedding: PathBuf,
        tokenizer: PathBuf,
    },
    /// 阿里 Qwen3-ASR 模型（前端卷积 + 编码器 + 解码器 + Tokenizer）
    Qwen3Asr {
        conv_frontend: PathBuf,
        encoder: PathBuf,
        decoder: PathBuf,
        tokenizer: PathBuf,
    },
    /// 小红书 FireRedASR2 模型（单模型 CTC 或 AED + 词表）
    FireRedAsr2 {
        model: Option<PathBuf>,
        encoder: Option<PathBuf>,
        decoder: Option<PathBuf>,
        tokens: PathBuf,
    },
}

/// 优先检查默认候选文件名中是否存在目标文件
fn resolve_candidate_file(dir: &Path, defaults: &[&str]) -> Option<PathBuf> {
    defaults.iter().map(|f| dir.join(f)).find(|p| p.exists())
}

/// 在指定目录中尝试探测 SenseVoice、Fun-ASR-Nano、Qwen3-ASR 或 FireRedASR2 架构的完整模型文件
fn detect_offline_files(path: &Path, model_type: OfflineModelType) -> Option<DetectedOfflineModel> {
    match model_type {
        OfflineModelType::SenseVoice => {
            let model = resolve_candidate_file(path, &["model.int8.onnx", "model.onnx"])?;
            let tokens = resolve_candidate_file(path, &["tokens.txt"])?;
            Some(DetectedOfflineModel::SenseVoice { model, tokens })
        }
        OfflineModelType::FunAsrNano => {
            let encoder_adaptor = resolve_candidate_file(
                path,
                &["encoder_adaptor.int8.onnx", "encoder_adaptor.onnx"],
            )?;
            let llm = resolve_candidate_file(path, &["llm.int8.onnx", "llm.onnx"])?;
            let embedding =
                resolve_candidate_file(path, &["embedding.int8.onnx", "embedding.onnx"])?;
            let tokenizer_dir = path.join("tokenizer");
            let tokenizer = if tokenizer_dir.is_dir() {
                tokenizer_dir
            } else {
                path.to_path_buf()
            };

            Some(DetectedOfflineModel::FunAsrNano {
                encoder_adaptor,
                llm,
                embedding,
                tokenizer,
            })
        }
        OfflineModelType::Qwen3Asr => {
            let conv_frontend =
                resolve_candidate_file(path, &["conv_frontend.int8.onnx", "conv_frontend.onnx"])?;
            let encoder = resolve_candidate_file(path, &["encoder.int8.onnx", "encoder.onnx"])?;
            let decoder = resolve_candidate_file(path, &["decoder.int8.onnx", "decoder.onnx"])?;
            let tokenizer_dir = path.join("tokenizer");
            let tokenizer = if tokenizer_dir.is_dir() {
                tokenizer_dir
            } else {
                path.to_path_buf()
            };

            Some(DetectedOfflineModel::Qwen3Asr {
                conv_frontend,
                encoder,
                decoder,
                tokenizer,
            })
        }
        OfflineModelType::FireRedAsr2 => {
            let tokens = resolve_candidate_file(path, &["tokens.txt"])?;
            let model = resolve_candidate_file(path, &["model.int8.onnx", "model.onnx"]);
            if let Some(m) = model {
                return Some(DetectedOfflineModel::FireRedAsr2 {
                    model: Some(m),
                    encoder: None,
                    decoder: None,
                    tokens,
                });
            }
            let encoder = resolve_candidate_file(path, &["encoder.int8.onnx", "encoder.onnx"])?;
            let decoder = resolve_candidate_file(path, &["decoder.int8.onnx", "decoder.onnx"])?;
            Some(DetectedOfflineModel::FireRedAsr2 {
                model: None,
                encoder: Some(encoder),
                decoder: Some(decoder),
                tokens,
            })
        }
    }
}

/// 校验离线 ASR 模型目录对指定模型架构是否文件完整
///
/// # 设计原理
/// - **实现初衷**：与运行期 [`detect_offline_files`] 共享同一套权重文件探测规则，消除下载器与加载器两套校验标准漂移风险。
/// - **核心优势**：保证安装状态判定与离线识别器真实可加载状态严格一致。
/// - **代价与局限**：需对目录内候选文件名执行少量元数据 `exists()` 探测。
pub(crate) fn is_offline_model_dir_complete(path: &Path, model_type: OfflineModelType) -> bool {
    path.is_dir() && detect_offline_files(path, model_type).is_some()
}

/// 根据模型目录内部权重文件布局及目录命名自动推断离线 ASR 架构类型
///
/// # 设计原理
/// - **实现初衷**：设置中心切换或导入本地自定义模型目录时，不能盲目保持上一个模型的 `model_type`，
///   需由真实文件结构自动判定架构以避免推理期参数错配。
/// - **核心优势**：优先依据唯一特征权重文件名探测，仅在权重格式共用的情况参考目录名兜底，
///   全无命中时默认回落到 `SenseVoice`。
pub fn infer_offline_model_type(path: &Path) -> OfflineModelType {
    if path.is_dir() {
        if detect_offline_files(path, OfflineModelType::Qwen3Asr).is_some() {
            return OfflineModelType::Qwen3Asr;
        }
        if detect_offline_files(path, OfflineModelType::FunAsrNano).is_some() {
            return OfflineModelType::FunAsrNano;
        }
        let lower_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if lower_name.contains("firered") || lower_name.contains("fire-red") {
            return OfflineModelType::FireRedAsr2;
        }
        if detect_offline_files(path, OfflineModelType::SenseVoice).is_some() {
            return OfflineModelType::SenseVoice;
        }
        if detect_offline_files(path, OfflineModelType::FireRedAsr2).is_some() {
            return OfflineModelType::FireRedAsr2;
        }
    }
    OfflineModelType::SenseVoice
}

/// 根据探测到的模型文件配置并创建 sherpa-onnx 离线识别器
fn create_offline_recognizer(
    detected: &DetectedOfflineModel,
    config: &OfflineAsrConfig,
) -> Option<OfflineRecognizer> {
    let mut rec_cfg = OfflineRecognizerConfig::default();
    rec_cfg.feat_config.sample_rate = 16000;
    rec_cfg.feat_config.feature_dim = 80;
    rec_cfg.decoding_method = Some("greedy_search".to_string());
    rec_cfg.model_config.num_threads = config.num_threads.max(1) as i32;
    rec_cfg.model_config.provider = Some("cpu".to_string());

    match detected {
        DetectedOfflineModel::SenseVoice { model, tokens } => {
            rec_cfg.model_config.tokens = Some(tokens.to_string_lossy().to_string());
            rec_cfg.model_config.sense_voice.model = Some(model.to_string_lossy().to_string());
            rec_cfg.model_config.sense_voice.language = Some(config.language.clone());
            rec_cfg.model_config.sense_voice.use_itn = true;
        }
        DetectedOfflineModel::FunAsrNano {
            encoder_adaptor,
            llm,
            embedding,
            tokenizer,
        } => {
            rec_cfg.model_config.funasr_nano.encoder_adaptor =
                Some(encoder_adaptor.to_string_lossy().to_string());
            rec_cfg.model_config.funasr_nano.llm = Some(llm.to_string_lossy().to_string());
            rec_cfg.model_config.funasr_nano.embedding =
                Some(embedding.to_string_lossy().to_string());
            rec_cfg.model_config.funasr_nano.tokenizer =
                Some(tokenizer.to_string_lossy().to_string());
            rec_cfg.model_config.funasr_nano.language = Some(config.language.clone());
            rec_cfg.model_config.funasr_nano.itn = 1;
        }
        DetectedOfflineModel::Qwen3Asr {
            conv_frontend,
            encoder,
            decoder,
            tokenizer,
        } => {
            rec_cfg.model_config.qwen3_asr.conv_frontend =
                Some(conv_frontend.to_string_lossy().to_string());
            rec_cfg.model_config.qwen3_asr.encoder = Some(encoder.to_string_lossy().to_string());
            rec_cfg.model_config.qwen3_asr.decoder = Some(decoder.to_string_lossy().to_string());
            rec_cfg.model_config.qwen3_asr.tokenizer =
                Some(tokenizer.to_string_lossy().to_string());
            rec_cfg.model_config.qwen3_asr.max_total_len = 512;
            rec_cfg.model_config.qwen3_asr.max_new_tokens = 128;
        }
        DetectedOfflineModel::FireRedAsr2 {
            model,
            encoder,
            decoder,
            tokens,
        } => {
            rec_cfg.model_config.tokens = Some(tokens.to_string_lossy().to_string());
            if let Some(m) = model {
                rec_cfg.model_config.fire_red_asr_ctc.model = Some(m.to_string_lossy().to_string());
            } else {
                if let Some(enc) = encoder {
                    rec_cfg.model_config.fire_red_asr.encoder =
                        Some(enc.to_string_lossy().to_string());
                }
                if let Some(dec) = decoder {
                    rec_cfg.model_config.fire_red_asr.decoder =
                        Some(dec.to_string_lossy().to_string());
                }
            }
        }
    }

    let recognizer = OfflineRecognizer::create(&rec_cfg)?;
    info!(
        "[本地 ASR] 成功创建 sherpa-onnx 原生离线识别引擎 ({})",
        config.model_type.as_str()
    );
    Some(recognizer)
}

/// 本地端侧离线语音识别器单次录音最大缓冲采样点数量（16kHz * 5分钟，与云端保持一致防止 OOM）
const MAX_OFFLINE_PCM_SAMPLES: usize = 16_000 * 60 * 5;

/// 本地端侧离线语音识别器（基于 sherpa-onnx 原生支持 SenseVoice-Small / Fun-ASR-Nano 权重）
pub struct OfflineVoiceRecognizer {
    /// 配置参数
    config: OfflineAsrConfig,
    /// 模型所在根目录解析后的绝对路径
    resolved_model_dir: Option<PathBuf>,
    /// 是否检测到完整的模型权重文件集合
    models_ready: bool,
    /// 活跃的 sherpa-onnx 离线识别器实例
    recognizer: Option<OfflineRecognizer>,
    /// 录音期间累计采集的 16kHz 单声道 PCM 样本
    accumulated_pcm: Vec<f32>,
}

impl OfflineVoiceRecognizer {
    /// 创建本地离线识别器并校验模型权重完整性
    pub fn new(config: OfflineAsrConfig) -> Self {
        let resolved_dir = resolve_asr_model_dir(config.model_dir.as_deref(), config.model_type);
        let mut models_ready = false;
        let mut recognizer = None;

        if let Some(ref path) = resolved_dir {
            if let Some(detected) = detect_offline_files(path, config.model_type) {
                info!(
                    "[本地 ASR] 检测到完整本地离线模型 (类型: {}, 目录: {:?})",
                    config.model_type.as_str(),
                    path
                );
                models_ready = true;
                if let Some(rec) = create_offline_recognizer(&detected, &config) {
                    recognizer = Some(rec);
                } else {
                    warn!(
                        "[本地 ASR] 模型文件已找到，但 sherpa-onnx 识别器创建失败；本次语音输入将明确报错而非伪造文本"
                    );
                }
            } else {
                warn!(
                    "[本地 ASR] 模型目录 {:?} 缺少 {} 必要权重文件",
                    path,
                    config.model_type.as_str()
                );
            }
        } else {
            info!("[本地 ASR] 未检测到已下载的本地 ASR 模型，语音输入将返回明确错误");
        }

        Self {
            config,
            resolved_model_dir: resolved_dir,
            models_ready,
            recognizer,
            accumulated_pcm: Vec::new(),
        }
    }

    /// 生成模型不可用错误
    fn unavailable_error(&self) -> VoiceError {
        let reason = match self.resolved_model_dir {
            Some(ref path) => format!(
                "模型目录 {} 缺少 {} 必要权重文件，或识别器初始化失败",
                path.display(),
                self.config.model_type.as_str()
            ),
            None => format!(
                "未找到本地离线 ASR 模型 ({})，请先在设置中心下载或导入模型",
                self.config.model_type.as_str()
            ),
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
            recognizer: None,
            accumulated_pcm: Vec::new(),
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
        self.recognizer.is_some()
    }

    fn feed_pcm(&mut self, samples: &[f32]) -> Result<Option<String>, VoiceError> {
        if self.recognizer.is_none() {
            return Err(self.unavailable_error());
        }
        // SenseVoice 与 Fun-ASR-Nano 均为整段非流式模型，录音期间缓冲累加样本（设 5 分钟防爆内存上限）
        let remaining = MAX_OFFLINE_PCM_SAMPLES.saturating_sub(self.accumulated_pcm.len());
        if remaining == 0 {
            return Ok(None);
        }
        let accept = remaining.min(samples.len());
        self.accumulated_pcm.extend_from_slice(&samples[..accept]);
        Ok(None)
    }

    fn finish(&mut self) -> Result<String, VoiceError> {
        let Some(recognizer) = &self.recognizer else {
            return Err(self.unavailable_error());
        };

        if self.accumulated_pcm.is_empty() {
            return Ok(String::new());
        }

        let stream = recognizer.create_stream();
        stream.accept_waveform(16000, &self.accumulated_pcm);
        recognizer.decode(&stream);

        let final_text = stream
            .get_result()
            .map(|r| r.text.trim().to_string())
            .unwrap_or_default();

        self.accumulated_pcm.clear();
        Ok(final_text)
    }

    fn reset(&mut self) {
        self.accumulated_pcm.clear();
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

    #[test]
    fn test_offline_model_type_properties() {
        assert!(OfflineModelType::SenseVoice.has_native_punctuation());
        assert!(OfflineModelType::FunAsrNano.has_native_punctuation());
        assert!(OfflineModelType::Qwen3Asr.has_native_punctuation());
        assert!(!OfflineModelType::FireRedAsr2.has_native_punctuation());
        assert_eq!(OfflineModelType::default(), OfflineModelType::SenseVoice);
    }

    #[test]
    fn test_is_offline_model_dir_complete_matches_all_architectures() {
        let dir = tempfile::tempdir().unwrap();
        let sv_dir = dir.path().join("sensevoice");
        std::fs::create_dir_all(&sv_dir).unwrap();
        assert!(!is_offline_model_dir_complete(
            &sv_dir,
            OfflineModelType::SenseVoice
        ));
        std::fs::write(sv_dir.join("model.int8.onnx"), b"stub").unwrap();
        assert!(!is_offline_model_dir_complete(
            &sv_dir,
            OfflineModelType::SenseVoice
        ));
        std::fs::write(sv_dir.join("tokens.txt"), b"stub").unwrap();
        assert!(is_offline_model_dir_complete(
            &sv_dir,
            OfflineModelType::SenseVoice
        ));

        let fr_dir = dir.path().join("firered");
        std::fs::create_dir_all(&fr_dir).unwrap();
        std::fs::write(fr_dir.join("tokens.txt"), b"stub").unwrap();
        std::fs::write(fr_dir.join("encoder.onnx"), b"stub").unwrap();
        assert!(!is_offline_model_dir_complete(
            &fr_dir,
            OfflineModelType::FireRedAsr2
        ));
        std::fs::write(fr_dir.join("decoder.onnx"), b"stub").unwrap();
        assert!(is_offline_model_dir_complete(
            &fr_dir,
            OfflineModelType::FireRedAsr2
        ));
    }
}
