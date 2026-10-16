//! DeepFilterNet3 深度神经网络降噪引擎封装
//!
//! # 设计原理
//! - **实现初衷**：针对机械键盘打字声与高动态杂音提供深度滤波增强；
//! - **自适应加速**：自动探测宿主机 AVX2 + FMA 向量指令集加速。

use log::{info, warn};
use std::path::PathBuf;

use super::NoiseSuppressor;

/// 内置高阶深度级 DeepFilterNet3 降噪器（基于纯 Rust DeepFilterNet3 架构）
pub struct DeepFilterSuppressor {
    /// DeepFilterNet3 模型引擎单例
    hush: Option<hush_vani::Hush>,
    /// 累积待处理的 16kHz PCM 样本
    pub(crate) in_buf: Vec<f32>,
    /// 降噪完毕的样本队列
    pub(crate) out_queue: Vec<f32>,
}

impl DeepFilterSuppressor {
    /// 创建 DeepFilterNet3 深度降噪器实例
    pub fn new() -> Self {
        let hush = match hush_vani::Hush::new() {
            Ok(h) => {
                info!("[音频前端] 成功初始化系统内置 DeepFilterNet3 深度降噪神经网络引擎 (AVX2+FMA 自动加速)");
                Some(h)
            }
            Err(e) => {
                warn!("[音频前端] 初始化 DeepFilterNet3 引擎失败: {}，降级运行", e);
                None
            }
        };

        Self {
            hush,
            in_buf: Vec::with_capacity(1600),
            out_queue: Vec::with_capacity(1600),
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
        self.in_buf.extend_from_slice(samples);

        // 每次按 160 采样点（10ms 整帧）处理
        let frame_size = 160;
        let available_frames = self.in_buf.len() / frame_size;
        if available_frames > 0 {
            let process_len = available_frames * frame_size;
            let chunk = &self.in_buf[..process_len];
            if let Ok(cleaned) = hush.enhance(chunk) {
                self.out_queue.extend_from_slice(&cleaned);
            }
            self.in_buf.drain(..process_len);
        }

        // 从降噪队列写回 samples
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
        self.in_buf.clear();
        self.out_queue.clear();
    }
}

/// 解析指定的 DeepFilterNet3 降噪模型路径
pub fn resolve_ns_model_path(custom_path: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = custom_path.filter(|s| !s.trim().is_empty()) {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".local/share/waylan/models/ns/deepfilternet3");
        if p.exists() {
            return Some(p);
        }
    }
    let sys_p = PathBuf::from("/usr/share/waylan/models/ns/deepfilternet3");
    if sys_p.exists() {
        return Some(sys_p);
    }
    let local_p = PathBuf::from("models/ns/deepfilternet3");
    if local_p.exists() {
        return Some(local_p);
    }
    let parent_p = PathBuf::from("../../models/ns/deepfilternet3");
    if parent_p.exists() {
        return Some(parent_p);
    }
    None
}
