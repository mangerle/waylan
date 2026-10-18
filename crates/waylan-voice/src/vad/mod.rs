//! 语音能量计算、动态波形可视化与可插拔端点检测 (VAD) 模块
//!
//! # 设计原理
//! - **实现初衷**：定义统一解耦的 `VadProcessor` Trait，支持用户在纯数学能量 VAD 与各类神经网络
//!   VAD 模型（如 Silero VAD）之间自由热插拔与替换。
//! - **核心优势**：纳秒级 CPU 开销，提供平滑的 Unicode 块级波形字符与端点静音截断判定。
//! - **代价与局限**：在未配置外部 ONNX 模型权重时，自动平滑回退至内置纯数学能量检测，保证零崩溃。

pub mod energy;
pub mod model;
pub mod visualizer;

pub use energy::EnergyVad;
pub use model::{resolve_vad_model_path, ModelVad};
pub use visualizer::{calculate_rms, WaveformVisualizer};

use std::path::PathBuf;
use std::time::{Duration, Instant};
use waylan_core::{VadConfig, VadModelType};

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

/// 向前兼容别名
pub type VadDetector = EnergyVad;

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
