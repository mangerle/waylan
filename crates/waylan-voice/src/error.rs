//! 微澜语音模块错误定义
//!
//! # 设计原理
//! - **实现初衷**：采用 `thiserror` 规范定义细粒度错误枚举，明确各个错误变体。
//! - **核心优势**：为调用方提供模式匹配能力，中文错误描述均附带动态上下文定位参数。
//! - **代价与局限**：底层硬件异常在边界处转换为内聚的领域错误，不向外泄漏底层细节。

use thiserror::Error;

/// 语音处理错误枚举
#[derive(Debug, Error)]
pub enum VoiceError {
    /// 查找或打开音频输入设备失败
    #[error("音频输入设备初始化失败，设备名称: {device}，原因: {message}")]
    DeviceInitFailed { device: String, message: String },

    /// 音频数据流启动失败
    #[error("音频捕获流启动失败，原因: {0}")]
    StreamStartFailed(String),

    /// 麦克风音频配置不支持（例如不支持采样率转换）
    #[error("不支持的音频格式配置，采样率: {sample_rate}，通道数: {channels}")]
    UnsupportedFormat { sample_rate: u32, channels: u16 },

    /// 音频数据通道已满或断开
    #[error("音频传输通道异常，原因: {0}")]
    ChannelError(String),

    /// ASR 语音识别引擎错误
    #[error("语音识别引擎处理失败，引擎类型: {engine_type}，原因: {message}")]
    RecognizerFailed {
        engine_type: String,
        message: String,
    },

    /// 创建语音后台常驻工作线程失败
    #[error("创建语音后台工作线程失败，原因: {0}")]
    WorkerThreadSpawnFailed(String),
}
