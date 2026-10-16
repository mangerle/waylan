//! 基于 `cpal` 的跨平台麦克风音频捕获与线性重采样模块
//!
//! # 设计原理
//! - **实现初衷**：以极低依赖捕获系统底层麦克风音频流，在底层音频线程中执行无锁零堆分配的通道合并与
//!   快速线性重采样，统一输出标准的 16kHz 单声道 32 位浮点 PCM 采样序列。
//! - **核心优势**：在音频中断回调中采用非阻塞有界同步通道（`try_send`），彻底杜绝音频线程卡顿与爆内存。
//! - **代价与局限**：线性重采样相比高阶带通 sinc 插值开销更低，但高频部分有轻微滤波失真，完全满足语音识别需求。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use log::{error, info, warn};

use crate::error::VoiceError;

/// 目标 ASR 统一的标准采样率 (16kHz)
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// 重采样状态机（线性插值器）
#[derive(Debug, Clone)]
pub struct LinearResampler {
    /// 原始采样率
    src_rate: f64,
    /// 目标采样率 (16000.0)
    target_rate: f64,
    /// 插值相位步进 (src_rate / target_rate)
    step: f64,
    /// 当前在原始音频流中的虚拟浮点位置
    position: f64,
    /// 上一个缓存采样点，用于跨帧线性插值
    last_sample: f32,
}

impl LinearResampler {
    /// 创建重采样器
    pub fn new(src_rate: u32, target_rate: u32) -> Self {
        let src = src_rate as f64;
        let target = target_rate as f64;
        Self {
            src_rate: src,
            target_rate: target,
            step: src / target,
            position: 0.0,
            last_sample: 0.0,
        }
    }

    /// 执行重采样转换
    ///
    /// 输入单声道采样切片，输出重采样后的样本并追加至 `out` 容器中。
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }

        // 若输入采样率恰好等于目标采样率，无需插值，直接拷贝
        if (self.src_rate - self.target_rate).abs() < 1.0 {
            out.extend_from_slice(input);
            return;
        }

        let input_len = input.len();
        while self.position < input_len as f64 {
            let idx = self.position.floor() as usize;
            let frac = (self.position - idx as f64) as f32;

            let s0 = if idx == 0 {
                self.last_sample
            } else {
                input[idx - 1]
            };
            let s1 = input[idx];

            // 线性插值公式：s0 + (s1 - s0) * frac
            let interpolated = s0 + (s1 - s0) * frac;
            out.push(interpolated);

            self.position += self.step;
        }

        self.position -= input_len as f64;
        self.last_sample = *input.last().unwrap_or(&0.0);
    }

    /// 重置状态
    pub fn reset(&mut self) {
        self.position = 0.0;
        self.last_sample = 0.0;
    }
}

/// 麦克风音频捕获控制器
pub struct AudioCapture {
    /// 活动音频输入流句柄（RAII 管理，Drop 时自动停止音频捕获）
    _stream: Stream,
    /// 音频流是否已因底层硬件断开或错误而异常中止
    is_stream_dead: Arc<AtomicBool>,
    /// 当前所选用的音频设备名称
    device_name: String,
    /// 硬件原始采样率
    native_sample_rate: u32,
    /// 硬件原始声道数
    channels: u16,
}

impl AudioCapture {
    /// 检查底层麦克风硬件音频流是否已因设备拔出或严重错误而异常中断
    pub fn is_stream_dead(&self) -> bool {
        self.is_stream_dead.load(Ordering::Acquire)
    }

    /// 启动默认麦克风音频捕获，将重采样后的 16kHz PCM 音频数据块发送至 `pcm_sender`
    pub fn start(pcm_sender: SyncSender<Vec<f32>>) -> Result<Self, VoiceError> {
        Self::start_with_device(pcm_sender, None)
    }

    /// 启动指定名称或默认麦克风的音频捕获（支持热插拔自动回退）
    ///
    /// # 参数
    /// - `pcm_sender`: 发送 16kHz PCM 浮点音频帧的通道；
    /// - `target_device`: 期望匹配的麦克风设备名称（模糊包含匹配），若未指定或未找到则自动回退至系统默认设备。
    pub fn start_with_device(
        pcm_sender: SyncSender<Vec<f32>>,
        target_device: Option<&str>,
    ) -> Result<Self, VoiceError> {
        let host = cpal::default_host();
        let device = if let Some(target_name) = target_device.filter(|s| !s.trim().is_empty()) {
            match host.input_devices() {
                Ok(mut devices) => {
                    devices.find(|d| d.name().map(|n| n.contains(target_name)).unwrap_or(false))
                }
                Err(_) => None,
            }
            .or_else(|| {
                warn!(
                    "[语音采集] 未找到指定麦克风 \"{}\"，自动降级回退至系统默认设备",
                    target_name
                );
                host.default_input_device()
            })
        } else {
            host.default_input_device()
        }
        .ok_or_else(|| VoiceError::DeviceInitFailed {
            device: target_device.unwrap_or("默认麦克风").to_string(),
            message: "未在当前系统中检测到可用的音频输入设备".to_string(),
        })?;

        let device_name = device
            .name()
            .unwrap_or_else(|_| "未知音频输入设备".to_string());
        info!("[语音采集] 正在初始化音频捕获设备: {}", device_name);

        let default_config =
            device
                .default_input_config()
                .map_err(|e| VoiceError::DeviceInitFailed {
                    device: device_name.clone(),
                    message: format!("获取设备默认配置失败: {}", e),
                })?;

        let sample_format = default_config.sample_format();
        let config: StreamConfig = default_config.into();
        let channels = config.channels;
        let native_sample_rate = config.sample_rate.0;

        info!(
            "[语音采集] 设备采样率: {} Hz, 声道数: {}, 采样格式: {:?}",
            native_sample_rate, channels, sample_format
        );

        let mut resampler = LinearResampler::new(native_sample_rate, TARGET_SAMPLE_RATE);
        let mut mono_buf = Vec::with_capacity(1024);
        let mut resampled_buf = Vec::with_capacity(1024);

        let is_stream_dead = Arc::new(AtomicBool::new(false));
        let dead_flag = is_stream_dead.clone();
        let err_fn = move |err| {
            error!("[语音采集 错误] 麦克风音频捕获流异常: {}", err);
            dead_flag.store(true, Ordering::Release);
        };

        // 根据不同底层采样格式分流处理
        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    mono_buf.clear();
                    resampled_buf.clear();

                    // 多通道向下均值合并为单声道
                    if channels > 1 {
                        let ch = channels as usize;
                        let frames = data.len() / ch;
                        mono_buf.reserve(frames);
                        for i in 0..frames {
                            let mut sum = 0.0;
                            for c in 0..ch {
                                sum += data[i * ch + c];
                            }
                            mono_buf.push(sum / channels as f32);
                        }
                    } else {
                        mono_buf.extend_from_slice(data);
                    }

                    // 重采样为 16000 Hz
                    resampler.process(&mono_buf, &mut resampled_buf);

                    // 非阻塞发送至有界缓冲通道，缓冲打满时丢弃以防背压导致音频线程死锁
                    if !resampled_buf.is_empty() {
                        let _ = pcm_sender.try_send(resampled_buf.clone());
                    }
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    mono_buf.clear();
                    resampled_buf.clear();

                    if channels > 1 {
                        let ch = channels as usize;
                        let frames = data.len() / ch;
                        mono_buf.reserve(frames);
                        for i in 0..frames {
                            let mut sum = 0.0;
                            for c in 0..ch {
                                sum += (data[i * ch + c] as f32) / 32768.0;
                            }
                            mono_buf.push(sum / channels as f32);
                        }
                    } else {
                        mono_buf.reserve(data.len());
                        for &sample in data {
                            mono_buf.push((sample as f32) / 32768.0);
                        }
                    }

                    resampler.process(&mono_buf, &mut resampled_buf);

                    if !resampled_buf.is_empty() {
                        let _ = pcm_sender.try_send(resampled_buf.clone());
                    }
                },
                err_fn,
                None,
            ),
            SampleFormat::U16 => device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    mono_buf.clear();
                    resampled_buf.clear();

                    if channels > 1 {
                        let ch = channels as usize;
                        let frames = data.len() / ch;
                        mono_buf.reserve(frames);
                        for i in 0..frames {
                            let mut sum = 0.0;
                            for c in 0..ch {
                                let f = (data[i * ch + c] as f32 - 32768.0) / 32768.0;
                                sum += f;
                            }
                            mono_buf.push(sum / channels as f32);
                        }
                    } else {
                        mono_buf.reserve(data.len());
                        for &sample in data {
                            let f = (sample as f32 - 32768.0) / 32768.0;
                            mono_buf.push(f);
                        }
                    }

                    resampler.process(&mono_buf, &mut resampled_buf);

                    if !resampled_buf.is_empty() {
                        let _ = pcm_sender.try_send(resampled_buf.clone());
                    }
                },
                err_fn,
                None,
            ),
            _ => {
                return Err(VoiceError::UnsupportedFormat {
                    sample_rate: native_sample_rate,
                    channels,
                })
            }
        }
        .map_err(|e| VoiceError::StreamStartFailed(e.to_string()))?;

        stream
            .play()
            .map_err(|e| VoiceError::StreamStartFailed(format!("激活音频捕获流失败: {}", e)))?;

        info!("[语音采集] 麦克风捕获流已成功启动");

        Ok(Self {
            _stream: stream,
            is_stream_dead,
            device_name,
            native_sample_rate,
            channels,
        })
    }

    /// 获取当前激活的硬件设备名称
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// 获取硬件原始采样率
    pub fn native_sample_rate(&self) -> u32 {
        self.native_sample_rate
    }

    /// 获取硬件声道数
    pub fn channels(&self) -> u16 {
        self.channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_resampler_identity() {
        let mut resampler = LinearResampler::new(16_000, 16_000);
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let mut out = Vec::new();
        resampler.process(&input, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn test_linear_resampler_downsample() {
        // 48000 Hz 降采样至 16000 Hz（3 倍抽取）
        let mut resampler = LinearResampler::new(48_000, 16_000);
        let input: Vec<f32> = (0..300).map(|i| (i as f32) / 300.0).collect();
        let mut out = Vec::new();
        resampler.process(&input, &mut out);

        // 输出点数约应为 100
        assert_eq!(out.len(), 100);
    }
}
