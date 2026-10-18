//! 纯数学能量端点检测器 (Energy-based VAD)
//!
//! # 设计原理
//! - **实现初衷**：在无神经网络模型依赖的情况下，提供最纯粹、零依赖的音频能量截断判定。
//! - **核心优势**：纯栈内浮点算术，纳秒级开销，对系统负载几乎为零。
//! - **代价与局限**：对背景稳态噪声敏感，复杂嘈杂环境下建议结合 AFE 降噪或 Silero 模型 VAD。

use super::visualizer::calculate_rms;
use super::VadProcessor;
use std::time::{Duration, Instant};

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
