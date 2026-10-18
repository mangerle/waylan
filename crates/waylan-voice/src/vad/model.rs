//! 神经网络 ONNX VAD 模型适配器
//!
//! # 设计原理
//! - **实现初衷**：接入行业高精度神经网络语音活动检测模型（如 Silero VAD），准确分离复杂背景噪音中的真实人声。
//! - **核心优势**：基于 sherpa-onnx 原生 C/C++ 绑定的跨平台推理加速；模型文件缺失时自动优雅降级为 EnergyVad，保证零崩溃。
//! - **代价与局限**：模型加载存在少许内存开销，单线程 CPU 推理约占 0.5%~1% CPU。

use super::energy::EnergyVad;
use super::VadProcessor;
use log::{info, warn};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// 解析指定的 VAD 模型文件路径（支持自定义路径、用户家目录与系统目录）
pub fn resolve_vad_model_path(custom_path: Option<&PathBuf>) -> Option<PathBuf> {
    if let Some(p) = custom_path {
        if p.exists() {
            return Some(p.clone());
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".local/share/waylan/models/vad/silero_vad.onnx");
        if p.exists() {
            return Some(p);
        }
    }
    let sys_p = PathBuf::from("/usr/share/waylan/models/vad/silero_vad.onnx");
    if sys_p.exists() {
        return Some(sys_p);
    }
    let local_p = PathBuf::from("models/vad/silero_vad.onnx");
    if local_p.exists() {
        return Some(local_p);
    }
    None
}

/// 神经网络 ONNX VAD 模型适配器（基于 sherpa-onnx Silero VAD 原生推理引擎）
pub struct ModelVad {
    /// 模型文件路径
    model_path: Option<PathBuf>,
    /// sherpa-onnx 真实 VAD 检测器实例
    active_detector: Option<VoiceActivityDetector>,
    /// 是否已开始说话
    has_speech_started: bool,
    /// 上一次听到人声的时间戳
    last_speech_time: Option<Instant>,
    /// 静音持续判定阈值
    silence_duration_threshold: Duration,
    /// 能量兜底检测器（模型文件未部署或加载失败时优雅降级）
    fallback_energy_vad: EnergyVad,
}

impl ModelVad {
    /// 创建模型端点检测器，支持自定义模型路径配置与自动多级路径探测
    pub fn new(
        model_path: Option<PathBuf>,
        speech_threshold: f32,
        silence_timeout: Duration,
    ) -> Self {
        let fallback = EnergyVad::new(speech_threshold, silence_timeout);
        let resolved_path = resolve_vad_model_path(model_path.as_ref());

        let mut active_detector = None;
        if let Some(ref path) = resolved_path {
            info!("[VAD 模型] 检测到神经网络 VAD 模型文件: {:?}", path);
            let vad_cfg = VadModelConfig {
                silero_vad: SileroVadModelConfig {
                    model: Some(path.to_string_lossy().to_string()),
                    threshold: speech_threshold,
                    min_silence_duration: silence_timeout.as_secs_f32(),
                    min_speech_duration: 0.25,
                    window_size: 512,
                    max_speech_duration: 30.0,
                },
                sample_rate: 16000,
                num_threads: 1,
                provider: Some("cpu".to_string()),
                ..Default::default()
            };

            if let Some(detector) = VoiceActivityDetector::create(&vad_cfg, 10.0) {
                info!("[VAD 模型] 成功创建 sherpa-onnx Silero VAD 原生检测器");
                active_detector = Some(detector);
            } else {
                warn!("[VAD 模型] 创建 Silero VAD 实例失败，正在自动回退至纯能量门限检测器");
            }
        } else {
            info!("[VAD 模型] 未检测到已下载的 Silero VAD 模型，使用内置纯数学能量检测保底");
        }

        Self {
            model_path: resolved_path,
            active_detector,
            has_speech_started: false,
            last_speech_time: None,
            silence_duration_threshold: silence_timeout,
            fallback_energy_vad: fallback,
        }
    }

    /// 获取当前加载的模型文件路径
    pub fn model_path(&self) -> Option<&PathBuf> {
        self.model_path.as_ref()
    }
}

impl VadProcessor for ModelVad {
    fn feed_chunk(&mut self, samples: &[f32], now: Instant) -> bool {
        if let Some(detector) = &self.active_detector {
            detector.accept_waveform(samples);
            let is_speech = detector.detected();

            if is_speech {
                self.has_speech_started = true;
                self.last_speech_time = Some(now);
                false
            } else if self.has_speech_started {
                // 已检测到完整语音段落，或当前静音持续时间已超时
                if !detector.is_empty() {
                    detector.clear();
                    return true;
                }
                if let Some(last_speech) = self.last_speech_time {
                    if now.duration_since(last_speech) >= self.silence_duration_threshold {
                        detector.clear();
                        return true;
                    }
                }
                false
            } else {
                false
            }
        } else {
            self.fallback_energy_vad.feed_chunk(samples, now)
        }
    }

    fn is_speech_started(&self) -> bool {
        if self.active_detector.is_some() {
            self.has_speech_started
        } else {
            self.fallback_energy_vad.is_speech_started()
        }
    }

    fn reset(&mut self) {
        if let Some(detector) = &self.active_detector {
            detector.clear();
            detector.reset();
        }
        self.has_speech_started = false;
        self.last_speech_time = None;
        self.fallback_energy_vad.reset();
    }
}
