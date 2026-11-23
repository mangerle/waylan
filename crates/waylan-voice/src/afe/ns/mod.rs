//! 人声智能降噪引擎 (Noise Suppression, NS) 模块
//!
//! # 架构设计
//! - `rnnoise`: 基于 `nnnoiseless` 的纯 Rust RNNoise 轻量级降噪器与 16k<->48k 采样率转换；
//! - `deepfilter`: 基于 `hush-vani` 的系统内置 DeepFilterNet3 高阶深度降噪引擎；
//! - `gtcrn`: 基于 `sherpa-onnx` 的 SOTA 级 GTCRN 卷积循环深度降噪器；
//! - `mod`: 统一 Trait 抽象、Bypass 直通实现与工厂构造路由。

pub mod deepfilter;
pub mod gtcrn;
pub mod rnnoise;

pub use deepfilter::{DeepFilterSuppressor, resolve_ns_model_path};
pub use gtcrn::{GtcrnSuppressor, resolve_gtcrn_model_path};
pub use rnnoise::RnnoiseSuppressor;

use log::info;
use waylan_core::{AudioFrontEndConfig, NsEngineType};

/// 人声降噪引擎统一 Trait 抽象
pub trait NoiseSuppressor: Send {
    /// 就地对 16kHz 单声道 PCM 浮点音频切片执行智能人声降噪
    fn process(&mut self, samples: &mut [f32]);

    /// 重置降噪内部循环记忆状态
    fn reset(&mut self);
}

/// 直通模式降噪器（Bypass，不执行任何处理）
#[derive(Debug, Clone, Default)]
pub struct BypassSuppressor;

impl BypassSuppressor {
    /// 创建直通降噪器（原样透传输入帧，用于降噪关闭或模型缺失时的空操作实现）
    pub fn new() -> Self {
        Self
    }
}

impl NoiseSuppressor for BypassSuppressor {
    fn process(&mut self, _samples: &mut [f32]) {
        // 直通不作修改
    }

    fn reset(&mut self) {}
}

/// 降噪器工厂构造函数
pub fn create_noise_suppressor(config: &AudioFrontEndConfig) -> Box<dyn NoiseSuppressor> {
    if !config.noise_suppression || config.ns_engine == NsEngineType::Bypass {
        info!("[音频前端] 用户配置关闭人声降噪，已启用直通透传模式");
        return Box::new(BypassSuppressor::new());
    }

    match config.ns_engine {
        NsEngineType::DeepFilterNet => {
            info!(
                "[音频前端] 激活系统内置高阶深度级 DeepFilterNet3 降噪引擎 (强力消除机械键盘打字声)"
            );
            Box::new(DeepFilterSuppressor::new())
        }
        NsEngineType::Gtcrn => {
            info!("[音频前端] 激活系统内置 SOTA 级 GTCRN 深度降噪引擎 (523KB 极速)");
            Box::new(GtcrnSuppressor::new())
        }
        NsEngineType::Rnnoise => {
            info!("[音频前端] 启用系统内置极轻量级 RNNoise 智能人声降噪引擎 (零资源占用)");
            Box::new(RnnoiseSuppressor::new())
        }
        NsEngineType::Bypass => Box::new(BypassSuppressor::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bypass_suppressor() {
        let mut suppressor = BypassSuppressor::new();
        let mut samples = vec![0.1f32, -0.2, 0.3, 0.4];
        let original = samples.clone();
        suppressor.process(&mut samples);
        assert_eq!(samples, original);
    }

    #[test]
    fn test_rnnoise_suppressor_processing() {
        let mut suppressor = RnnoiseSuppressor::new();

        // 连续送入 1600 个采样点（100ms @ 16kHz）的正弦纯音
        let mut samples: Vec<f32> = (0..1600)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 16000.0).sin() * 0.5)
            .collect();

        suppressor.process(&mut samples);

        // 降噪后样本必须在合法 [-1.0, 1.0] 范围内，且不包含 NaN 或 Inf
        for &s in &samples {
            assert!(!s.is_nan(), "降噪输出不得包含 NaN");
            assert!(!s.is_infinite(), "降噪输出不得包含 Inf");
            assert!(
                (-1.05..=1.05).contains(&s),
                "降噪输出必须处于有效幅值范围，当前: {}",
                s
            );
        }
    }

    #[test]
    fn test_rnnoise_suppressor_reset() {
        let mut suppressor = RnnoiseSuppressor::new();
        let mut samples = vec![0.5f32; 320];
        suppressor.process(&mut samples);

        suppressor.reset();
        assert_eq!(suppressor.last_16k_sample(), 0.0);
        assert!(suppressor.in_48k_buf.is_empty());
        assert!(suppressor.out_16k_queue.is_empty());
    }

    #[test]
    fn test_create_noise_suppressor_factory() {
        let cfg = AudioFrontEndConfig {
            noise_suppression: false,
            ..Default::default()
        };
        let mut supp = create_noise_suppressor(&cfg);
        let mut s = vec![0.3f32; 10];
        supp.process(&mut s);
        assert_eq!(s[0], 0.3f32);
    }

    #[test]
    fn test_resolve_ns_model_path_detection() {
        // 验证可成功解析当前工程目录下的 models/ns/deepfilternet3
        let resolved = resolve_ns_model_path(None);
        assert!(
            resolved.is_some(),
            "必须能成功探测到本地内置的 deepfilternet3 降噪模型目录"
        );
        let path = resolved.unwrap();
        assert!(path.join("config.ini").exists());
        assert!(path.join("enc.onnx").exists());

        // 验证自定义有效路径优先级
        let custom_str = path.to_string_lossy();
        let custom = resolve_ns_model_path(Some(&custom_str));
        assert_eq!(custom, Some(path));
    }

    #[test]
    fn test_deep_filter_suppressor_processing_and_reset() {
        let mut suppressor = DeepFilterSuppressor::new();
        let mut samples: Vec<f32> = (0..1600)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 16000.0).sin() * 0.5)
            .collect();
        suppressor.process(&mut samples);
        for &s in &samples {
            assert!(!s.is_nan());
            assert!(!s.is_infinite());
            assert!((-1.05..=1.05).contains(&s));
        }

        suppressor.reset();
        assert!(suppressor.in_buf.is_empty());
        assert!(suppressor.out_queue.is_empty());
    }

    #[test]
    fn test_gtcrn_suppressor_processing_and_reset() {
        let mut suppressor = GtcrnSuppressor::new();
        // 构造 100ms 的 16kHz 正弦波音频
        let mut samples: Vec<f32> = (0..1600)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 16000.0).sin() * 0.5)
            .collect();
        suppressor.process(&mut samples);
        for &s in &samples {
            assert!(!s.is_nan(), "GTCRN 降噪输出不得包含 NaN");
            assert!(!s.is_infinite(), "GTCRN 降噪输出不得包含 Inf");
            assert!((-1.05..=1.05).contains(&s));
        }

        suppressor.reset();
        assert!(suppressor.in_buf.is_empty());
        assert!(suppressor.out_queue.is_empty());
    }
}
