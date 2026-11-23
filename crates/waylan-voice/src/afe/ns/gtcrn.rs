//! 基于 sherpa-onnx 原生驱动的 GTCRN 卷积循环深度降噪器
//!
//! # 设计原理
//! - **实现初衷**：以 523KB 的轻量权重在端侧提供针对各类稳态白噪与混响声的深度抑制；
//! - **原生流式**：直接在 16kHz 单声道 PCM 浮点音频流上运行。

use log::{info, warn};
use std::path::PathBuf;

use super::NoiseSuppressor;

/// 基于 sherpa-onnx 原生驱动的 SOTA 级 GTCRN 卷积循环深度降噪器 (523KB)
pub struct GtcrnSuppressor {
    denoiser: Option<sherpa_onnx::OnlineSpeechDenoiser>,
    pub(crate) in_buf: Vec<f32>,
    pub(crate) out_queue: Vec<f32>,
}

impl GtcrnSuppressor {
    /// 创建 GTCRN 降噪器实例
    pub fn new() -> Self {
        let model_path = resolve_gtcrn_model_path();
        let denoiser = if let Some(ref p) = model_path {
            let mut cfg = sherpa_onnx::OnlineSpeechDenoiserConfig::default();
            cfg.model.gtcrn.model = Some(p.to_string_lossy().to_string());
            cfg.model.num_threads = 1;
            cfg.model.provider = Some("cpu".to_string());
            match sherpa_onnx::OnlineSpeechDenoiser::create(&cfg) {
                Some(d) => {
                    info!(
                        "[音频前端] 成功初始化系统内置 GTCRN 深度降噪神经网络引擎 (523KB, 模型: {:?})",
                        p
                    );
                    Some(d)
                }
                None => {
                    warn!("[音频前端] 初始化 GTCRN 降噪模型失败，将回退为直通模式");
                    None
                }
            }
        } else {
            warn!(
                "[音频前端] 未找到 GTCRN 降噪模型文件 (models/ns/gtcrn/gtcrn_simple.onnx)，将回退为直通模式"
            );
            None
        };

        Self {
            denoiser,
            in_buf: Vec::with_capacity(1600),
            out_queue: Vec::with_capacity(1600),
        }
    }
}

impl Default for GtcrnSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseSuppressor for GtcrnSuppressor {
    fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        let Some(ref denoiser) = self.denoiser else {
            return;
        };

        // 将切片送入流式推理，GTCRN 原生在 16kHz 单声道下工作
        let denoised = denoiser.run(samples, 16000);
        if !denoised.samples.is_empty() {
            self.out_queue.extend(denoised.samples);
        }

        let needed = samples.len();
        if self.out_queue.len() >= needed {
            samples.copy_from_slice(&self.out_queue[..needed]);
            self.out_queue.drain(..needed);
        } else {
            // 初始冷启动期间队列尚未排满，将已有样本填入，不足部分以静音垫底，严禁残留原始样本拼接导致时域阶跃爆音
            let available = self.out_queue.len();
            samples[..available].copy_from_slice(&self.out_queue[..available]);
            samples[available..].fill(0.0);
            self.out_queue.clear();
        }
    }

    fn reset(&mut self) {
        if let Some(ref denoiser) = self.denoiser {
            denoiser.reset();
        }
        self.in_buf.clear();
        self.out_queue.clear();
    }
}

/// 解析 GTCRN 模型文件路径（支持 XDG 用户目录、系统共享目录与源码本地目录）
pub fn resolve_gtcrn_model_path() -> Option<PathBuf> {
    waylan_core::config::resolve_model_path(None, &["ns/gtcrn/gtcrn_simple.onnx"])
}
