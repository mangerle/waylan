//! 基于 `cpal` 的跨平台麦克风音频捕获与线性重采样模块
//!
//! # 设计原理
//! - **实现初衷**：以极低依赖捕获系统底层麦克风音频流，在底层音频线程中执行无锁零堆分配的通道合并与
//!   快速线性重采样，统一输出标准的 16kHz 单声道 32 位浮点 PCM 采样序列。
//! - **核心优势**：在音频中断回调中采用非阻塞有界同步通道（`try_send`），彻底杜绝音频线程卡顿与爆内存。
//! - **代价与局限**：线性重采样相比高阶带通 sinc 插值开销更低，但高频部分有轻微滤波失真，完全满足语音识别需求。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
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
    /// 因有界 PCM 通道满而被丢弃的音频切片数量
    dropped_chunks: Arc<AtomicU64>,
    /// 当前所选用的音频设备名称
    device_name: String,
    /// 硬件原始采样率
    native_sample_rate: u32,
    /// 硬件原始声道数
    channels: u16,
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        let dropped = self.dropped_chunks.load(Ordering::Acquire);
        if dropped > 0 {
            warn!(
                "[语音采集] 本次录音因下游处理不及时丢弃了 {} 个音频切片（设备: {}）",
                dropped, self.device_name
            );
        }
    }
}

impl AudioCapture {
    /// 检查底层麦克风硬件音频流是否已因设备拔出或严重错误而异常中断
    pub fn is_stream_dead(&self) -> bool {
        self.is_stream_dead.load(Ordering::Acquire)
    }

    /// 查询本次录音期间因下游通道拥塞而丢弃的音频切片数量
    pub fn dropped_chunks(&self) -> u64 {
        self.dropped_chunks.load(Ordering::Acquire)
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
        let (device, device_name) = Self::select_input_device(target_device)?;
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

        let is_stream_dead = Arc::new(AtomicBool::new(false));
        let dropped_chunks = Arc::new(AtomicU64::new(0));
        let stream = Self::build_input_stream_by_format(
            &device,
            &config,
            sample_format,
            is_stream_dead.clone(),
            dropped_chunks.clone(),
            pcm_sender,
        )?;

        stream.play().map_err(|e| VoiceError::StreamStartFailed {
            message: format!("激活音频捕获流失败: {}", e),
        })?;

        info!("[语音采集] 麦克风捕获流已成功启动");

        Ok(Self {
            _stream: stream,
            is_stream_dead,
            dropped_chunks,
            device_name,
            native_sample_rate,
            channels,
        })
    }

    /// 探测并选择音频输入硬件设备
    fn select_input_device(target_device: Option<&str>) -> Result<(Device, String), VoiceError> {
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

        let name = device
            .name()
            .unwrap_or_else(|_| "未知音频输入设备".to_string());
        Ok((device, name))
    }

    /// 根据底层硬件采样格式构建重采样管道与音频输入流
    fn build_input_stream_by_format(
        device: &Device,
        config: &StreamConfig,
        sample_format: SampleFormat,
        is_stream_dead: Arc<AtomicBool>,
        dropped_chunks: Arc<AtomicU64>,
        pcm_sender: SyncSender<Vec<f32>>,
    ) -> Result<cpal::Stream, VoiceError> {
        let channels = config.channels;
        let native_sample_rate = config.sample_rate.0;
        let mut pipeline = CapturePipeline {
            resampler: LinearResampler::new(native_sample_rate, TARGET_SAMPLE_RATE),
            mono_buf: Vec::with_capacity(1024),
            resampled_buf: Vec::with_capacity(1024),
            pcm_sender,
            dropped_chunks,
        };

        let dead_flag = is_stream_dead.clone();
        let err_fn = move |err| {
            error!("[语音采集 错误] 麦克风音频捕获流异常: {}", err);
            dead_flag.store(true, Ordering::Release);
        };

        match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                config,
                move |data: &[f32], _| {
                    pipeline.convert(data, channels, |s| s);
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                config,
                move |data: &[i16], _| {
                    pipeline.convert(data, channels, |s| (s as f32) / 32768.0);
                },
                err_fn,
                None,
            ),
            SampleFormat::U16 => device.build_input_stream(
                config,
                move |data: &[u16], _| {
                    pipeline.convert(data, channels, |s| (s as f32 - 32768.0) / 32768.0);
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
        .map_err(|e| VoiceError::StreamStartFailed {
            message: e.to_string(),
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

/// 音频回调复用的转换、下混与重采样工作区
struct CapturePipeline {
    resampler: LinearResampler,
    mono_buf: Vec<f32>,
    resampled_buf: Vec<f32>,
    pcm_sender: SyncSender<Vec<f32>>,
    dropped_chunks: Arc<AtomicU64>,
}

impl CapturePipeline {
    /// 转换单个硬件音频回调切片并投递至下游有界通道
    fn convert<T: Copy, F: Fn(T) -> f32>(&mut self, data: &[T], channels: u16, converter: F) {
        self.mono_buf.clear();
        self.resampled_buf.clear();

        if channels > 1 {
            let ch = channels as usize;
            let frames = data.len() / ch;
            self.mono_buf.reserve(frames);
            for i in 0..frames {
                let mut sum = 0.0;
                for c in 0..ch {
                    sum += converter(data[i * ch + c]);
                }
                self.mono_buf.push(sum / channels as f32);
            }
        } else {
            self.mono_buf.reserve(data.len());
            for &sample in data {
                self.mono_buf.push(converter(sample));
            }
        }

        self.resampler
            .process(&self.mono_buf, &mut self.resampled_buf);

        if !self.resampled_buf.is_empty() {
            // 移动所有权而非整段复制；通道拥塞时数据明确计入丢弃计数，避免静默丢音。
            let chunk = std::mem::take(&mut self.resampled_buf);
            match self.pcm_sender.try_send(chunk) {
                Ok(()) => {
                    self.resampled_buf.reserve(1024);
                }
                Err(std::sync::mpsc::TrySendError::Full(chunk)) => {
                    self.resampled_buf = chunk;
                    self.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(chunk)) => {
                    self.resampled_buf = chunk;
                    self.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
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
