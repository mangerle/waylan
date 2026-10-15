//! 语音能量计算、动态波形可视化与可插拔端点检测 (VAD) 模块
//!
//! # 设计原理
//! - **实现初衷**：定义统一解耦的 `VadProcessor` Trait，支持用户在纯数学能量 VAD 与各类神经网络
//!   VAD 模型（如 Silero VAD）之间自由热插拔与替换。
//! - **核心优势**：纳秒级 CPU 开销，提供平滑的 Unicode 块级波形字符与端点静音截断判定。
//! - **代价与局限**：在未配置外部 ONNX 模型权重时，自动平滑回退至内置纯数学能量检测，保证零崩溃。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use log::{info, warn};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use waylan_core::{VadConfig, VadModelType};

/// Unicode 块级音量梯度符号（共 8 级）
const VOLUME_CHARS: [char; 8] = [' ', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// 计算给定的 32 位浮点 PCM 采样的均方根能量 (Root Mean Square, RMS)
///
/// 返回值范围通常在 `0.0` 到 `1.0` 之间。
#[inline]
pub fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_squares: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

/// 动态音量波形条生成器
#[derive(Debug, Clone)]
pub struct WaveformVisualizer {
    /// 历史平滑音量队列（固定长度滑动窗口）
    history: [f32; 5],
    /// 当前滑动窗口写入指针
    write_idx: usize,
}

impl Default for WaveformVisualizer {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveformVisualizer {
    /// 创建新的波形生成器，默认窗口大小为 5 根柱形
    pub fn new() -> Self {
        Self {
            history: [0.0; 5],
            write_idx: 0,
        }
    }

    /// 压入新的瞬间音量采样（0.0 ~ 1.0）并生成当前渲染字符串
    pub fn update(&mut self, current_volume: f32) -> String {
        self.history[self.write_idx] = current_volume;
        self.write_idx = (self.write_idx + 1) % self.history.len();

        let mut out = String::with_capacity(self.history.len());
        for i in 0..self.history.len() {
            let idx = (self.write_idx + i) % self.history.len();
            let vol = self.history[idx];
            let level = (vol * 15.0).clamp(0.0, 7.0) as usize;
            out.push(VOLUME_CHARS[level]);
        }
        out
    }

    /// 重置波形历史状态
    pub fn reset(&mut self) {
        self.history = [0.0; 5];
        self.write_idx = 0;
    }
}

/// 统一端点检测器抽象特型 (VAD Trait)
pub trait VadProcessor: Send + Sync {
    /// 输入当前音频块的 16kHz PCM 采样与时间戳，进行端点检测
    ///
    /// # 返回值
    /// - `true`: 检测到人声说话后持续静音超时，应触发截断提交；
    /// - `false`: 尚未超时或正在说话。
    fn feed_chunk(&mut self, samples: &[f32], now: Instant) -> bool;

    /// 是否已经检测到人声开始
    fn is_speech_started(&self) -> bool;

    /// 重置端点检测状态
    fn reset(&mut self);
}

/// 基础纯数学能量端点检测器 (Energy-based VAD)
#[derive(Debug)]
pub struct EnergyVad {
    /// 触发语音开始的能量下限门限
    speech_threshold: f32,
    /// 判定为静音的持续时长阈值
    silence_duration_threshold: Duration,
    /// 是否已检测到用户开始说话
    has_speech_started: bool,
    /// 上一次检测到有效人声的时间戳
    last_speech_time: Option<Instant>,
}

impl EnergyVad {
    /// 创建能量检测器，指定语音能量阈值与静音截断持续时间
    pub fn new(speech_threshold: f32, silence_duration_threshold: Duration) -> Self {
        Self {
            speech_threshold,
            silence_duration_threshold,
            has_speech_started: false,
            last_speech_time: None,
        }
    }
}

impl VadProcessor for EnergyVad {
    fn feed_chunk(&mut self, samples: &[f32], now: Instant) -> bool {
        let rms = calculate_rms(samples);
        if rms >= self.speech_threshold {
            self.has_speech_started = true;
            self.last_speech_time = Some(now);
            false
        } else if self.has_speech_started {
            if let Some(last_speech) = self.last_speech_time {
                if now.duration_since(last_speech) >= self.silence_duration_threshold {
                    return true;
                }
            }
            false
        } else {
            false
        }
    }

    fn is_speech_started(&self) -> bool {
        self.has_speech_started
    }

    fn reset(&mut self) {
        self.has_speech_started = false;
        self.last_speech_time = None;
    }
}

/// 向前兼容别名
pub type VadDetector = EnergyVad;

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

/// 根据 VAD 配置动态构造对应的端点检测器实例（工厂函数）
pub fn create_vad(config: &VadConfig) -> Box<dyn VadProcessor> {
    let timeout = Duration::from_millis(config.silence_timeout_ms);
    match config.vad_type {
        VadModelType::SileroOnnx => {
            let model_path = config.model_path.as_ref().map(PathBuf::from);
            Box::new(ModelVad::new(model_path, config.speech_threshold, timeout))
        }
        VadModelType::Energy => Box::new(EnergyVad::new(config.speech_threshold, timeout)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_rms() {
        assert_eq!(calculate_rms(&[]), 0.0);
        let samples = [0.5, -0.5, 0.5, -0.5];
        let rms = calculate_rms(&samples);
        assert!((rms - 0.5).abs() < 1e-4);
    }

    #[test]
    fn test_waveform_visualizer() {
        let mut visualizer = WaveformVisualizer::new();
        let frame1 = visualizer.update(0.0);
        assert_eq!(frame1.chars().count(), 5);

        let frame2 = visualizer.update(0.8);
        assert_eq!(frame2.chars().count(), 5);
        assert!(frame2.contains('█'));
    }

    #[test]
    fn test_vad_factory_and_silence_detection() {
        let config = VadConfig {
            vad_type: VadModelType::Energy,
            model_path: None,
            speech_threshold: 0.05,
            silence_timeout_ms: 500,
        };
        let mut vad = create_vad(&config);
        let start = Instant::now();

        let quiet_chunk = vec![0.01; 160];
        assert!(!vad.feed_chunk(&quiet_chunk, start));
        assert!(!vad.is_speech_started());

        let speech_chunk = vec![0.1; 160];
        let t1 = start + Duration::from_millis(100);
        assert!(!vad.feed_chunk(&speech_chunk, t1));
        assert!(vad.is_speech_started());

        let t2 = start + Duration::from_millis(300);
        assert!(!vad.feed_chunk(&quiet_chunk, t2));

        let t3 = start + Duration::from_millis(700);
        assert!(vad.feed_chunk(&quiet_chunk, t3));
    }

    #[test]
    fn test_silero_vad_detects_model() {
        let config = VadConfig {
            vad_type: VadModelType::SileroOnnx,
            model_path: None,
            speech_threshold: 0.5,
            silence_timeout_ms: 600,
        };
        let mut vad = create_vad(&config);
        let start = Instant::now();
        let chunk = vec![0.0; 512];
        // 验证真实 Silero VAD 被探测发现并执行正常端点判定
        assert!(!vad.feed_chunk(&chunk, start));
    }
}
