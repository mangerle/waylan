//! 内置纯 Rust RNNoise 人声降噪器（基于 `nnnoiseless`）
//!
//! # 设计原理
//! - **实现初衷**：利用轻量级循环神经网络模型在 48kHz 下实时滤除背景平稳与突发杂音；
//! - **采样适配**：内建 16kHz 与 48kHz 之间的 3 倍双向线性插值与均值抗混叠重采样状态机。

use std::collections::VecDeque;

use super::{NoiseSuppressor, drain_output_queue};

/// 内置纯 Rust RNNoise 降噪器（基于 `nnnoiseless`）
pub struct RnnoiseSuppressor {
    /// 神经网络状态单例
    denoiser: Box<nnnoiseless::DenoiseState<'static>>,
    /// 上一个 16kHz 采样点，用于 1:3 线性插值
    last_16k_sample: f32,
    /// 累积待送入 48kHz 降噪网络的缓冲区（满 480 点处理一帧）
    pub(crate) in_48k_buf: VecDeque<f32>,
    /// 降噪完毕已转换回 16kHz 的暂存队列
    pub(crate) out_16k_queue: VecDeque<f32>,
    /// 用于送入 process_frame 的输入帧缓冲 (480 点)
    frame_in: [f32; nnnoiseless::DenoiseState::FRAME_SIZE],
    /// 用于接收 process_frame 的输出帧缓冲 (480 点)
    frame_out: [f32; nnnoiseless::DenoiseState::FRAME_SIZE],
}

impl RnnoiseSuppressor {
    /// 创建纯 Rust RNNoise 降噪器
    pub fn new() -> Self {
        let mut denoiser = nnnoiseless::DenoiseState::new();
        // 丢弃第一帧瞬态初始响应
        let dummy_in = [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE];
        let mut dummy_out = [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE];
        let _ = denoiser.process_frame(&mut dummy_out, &dummy_in);

        Self {
            denoiser,
            last_16k_sample: 0.0,
            in_48k_buf: VecDeque::with_capacity(1024),
            out_16k_queue: VecDeque::with_capacity(1024),
            frame_in: [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE],
            frame_out: [0.0f32; nnnoiseless::DenoiseState::FRAME_SIZE],
        }
    }

    /// 执行 16kHz 到 48kHz 的 3 倍线性插值上采样并追加至 in_48k_buf
    fn upsample_16k_to_48k(&mut self, input: &[f32]) {
        for &s in input {
            let s_prev = self.last_16k_sample;
            // 3 倍插值：s_prev + 1/3*(s - s_prev), s_prev + 2/3*(s - s_prev), s
            let delta = s - s_prev;
            self.in_48k_buf.push_back(s_prev + delta * (1.0 / 3.0));
            self.in_48k_buf.push_back(s_prev + delta * (2.0 / 3.0));
            self.in_48k_buf.push_back(s);
            self.last_16k_sample = s;
        }
    }

    /// 循环消化 48kHz 缓冲区中的整帧并降噪
    fn process_available_frames(&mut self) {
        let frame_size = nnnoiseless::DenoiseState::FRAME_SIZE;

        while self.in_48k_buf.len() >= frame_size {
            // 拷贝并消费 480 个样本，放缩至 16-bit 浮点范围 [-32768, 32767]
            for (target, sample) in self
                .frame_in
                .iter_mut()
                .zip(self.in_48k_buf.drain(..frame_size))
            {
                *target = sample * 32767.0;
            }

            // 神经网络帧级推理
            let _ = self
                .denoiser
                .process_frame(&mut self.frame_out, &self.frame_in);

            // 3:1 抗混叠均值下采样回到 16kHz（480 点 -> 160 点）
            let out_16k_samples = frame_size / 3;
            for i in 0..out_16k_samples {
                let idx = i * 3;
                let sum = self.frame_out[idx] + self.frame_out[idx + 1] + self.frame_out[idx + 2];
                let downsampled = (sum / 3.0) / 32767.0;
                self.out_16k_queue.push_back(downsampled.clamp(-1.0, 1.0));
            }
        }
    }

    /// 获取上一个 16k 采样点记录（用于单测验证重置）
    #[cfg(test)]
    pub(crate) fn last_16k_sample(&self) -> f32 {
        self.last_16k_sample
    }
}

impl Default for RnnoiseSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseSuppressor for RnnoiseSuppressor {
    fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }

        // 1. 线性插值上采样为 48kHz 并存入缓冲
        self.upsample_16k_to_48k(samples);

        // 2. 批量处理所有凑满 480 样本（10ms）的整帧
        self.process_available_frames();

        // 3. 从降噪完成队列写回对应长度样本
        drain_output_queue(&mut self.out_16k_queue, samples);
    }

    fn reset(&mut self) {
        self.denoiser = nnnoiseless::DenoiseState::new();
        self.last_16k_sample = 0.0;
        self.in_48k_buf.clear();
        self.out_16k_queue.clear();
    }
}
