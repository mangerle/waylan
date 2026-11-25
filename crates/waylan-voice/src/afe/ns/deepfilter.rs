//! DeepFilterNet3 深度神经网络降噪引擎封装
//!
//! # 设计原理
//! - **实现初衷**：针对机械键盘打字声与高动态杂音提供深度滤波增强；
//! - **自适应加速**：自动探测宿主机 AVX2 + FMA 向量指令集加速。

use std::collections::VecDeque;
use std::path::PathBuf;

use log::{info, warn};

use super::{NoiseSuppressor, drain_output_queue};

/// 内置高阶深度级 DeepFilterNet3 降噪器（基于纯 Rust DeepFilterNet3 架构）
pub struct DeepFilterSuppressor {
    /// DeepFilterNet3 模型引擎单例
    hush: Option<hush_vani::Hush>,
    /// 累积待处理的 16kHz PCM 样本
    pub(crate) in_buf: VecDeque<f32>,
    /// 降噪完毕的样本队列
    pub(crate) out_queue: VecDeque<f32>,
}

impl DeepFilterSuppressor {
    /// 创建 DeepFilterNet3 深度降噪器实例
    pub fn new() -> Self {
        let hush = match hush_vani::Hush::new() {
            Ok(h) => {
                info!(
                    "[音频前端] 成功初始化系统内置 DeepFilterNet3 深度降噪神经网络引擎 (AVX2+FMA 自动加速)"
                );
                Some(h)
            }
            Err(e) => {
                warn!("[音频前端] 初始化 DeepFilterNet3 引擎失败: {}，降级运行", e);
                None
            }
        };

        Self {
            hush,
            in_buf: VecDeque::with_capacity(1600),
            out_queue: VecDeque::with_capacity(1600),
        }
    }
}

impl Default for DeepFilterSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseSuppressor for DeepFilterSuppressor {
    fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        let Some(ref hush) = self.hush else {
            return;
        };

        // 压入当前切片
        self.in_buf.extend(samples.iter().copied());

        // 每次按 160 采样点（10ms 整帧）处理
        const FRAME_SIZE: usize = 160;
        let available_frames = self.in_buf.len() / FRAME_SIZE;
        if available_frames > 0 {
            let process_len = available_frames * FRAME_SIZE;
            let chunk = &self.in_buf.make_contiguous()[..process_len];
            if let Ok(cleaned) = hush.enhance(chunk) {
                self.out_queue.extend(cleaned);
            }
            self.in_buf.drain(..process_len);
        }

        // 从降噪队列写回 samples
        drain_output_queue(&mut self.out_queue, samples);
    }

    fn reset(&mut self) {
        self.in_buf.clear();
        self.out_queue.clear();
    }
}

/// 解析指定的 DeepFilterNet3 降噪模型路径
pub fn resolve_ns_model_path(custom_path: Option<&str>) -> Option<PathBuf> {
    let custom = custom_path
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(std::path::Path::new);
    waylan_core::config::resolve_model_path(custom, &["ns/deepfilternet3"])
}
