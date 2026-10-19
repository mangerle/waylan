//! 微澜 IBus 会话错误类型定义
//!
//! # 设计原理
//! - **实现初衷**：为 IBus 会话、总线连接、服务注册与事件监控提供细粒度的强类型错误定义。
//! - **核心优势**：基于 `thiserror` 实现，提供带动态上下文的中文错误描述，避免无类型的 `Box<dyn Error>`。
//! - **代价与局限**：调用方可解构匹配精确的错误变体，对未预期的底层变动进行了合理收敛。

use thiserror::Error;

/// 微澜 IBus 会话生命周期与协议交互错误
#[derive(Debug, Error)]
pub enum IbusError {
    /// 未发现活跃的 IBus 会话总线地址
    #[error("未检测到活跃的 IBus 会话总线地址，请确认 ibus-daemon 正在运行")]
    AddressNotFound,

    /// D-Bus 通信失败
    #[error("IBus D-Bus 通信失败，原因: {reason}")]
    Dbus {
        /// 具体失败原因
        reason: String,
    },

    /// 系统 I/O 错误
    #[error("IBus 底层 I/O 操作失败，原因: {reason}")]
    Io {
        /// 具体失败原因
        reason: String,
    },

    /// 通用会话交互与协议调用失败
    #[error("IBus 会话交互失败，阶段: {stage}，原因: {reason}")]
    Session {
        /// 发生阶段或操作描述
        stage: &'static str,
        /// 具体失败原因
        reason: String,
    },
}

impl From<zbus::Error> for IbusError {
    fn from(err: zbus::Error) -> Self {
        Self::Dbus {
            reason: err.to_string(),
        }
    }
}

impl From<std::io::Error> for IbusError {
    fn from(err: std::io::Error) -> Self {
        Self::Io {
            reason: err.to_string(),
        }
    }
}
