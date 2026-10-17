//! 微澜守护进程错误体系与异常类型定义
//!
//! # 设计原理
//! - **实现初衷**：为微澜守护进程、CLI 命令分发、日志初始化以及 Wayland/IBus 会话调度提供强类型错误枚举。
//! - **核心优势**：消除顶层散落的无类型 `Box<dyn Error>`，提供动态中文错误上下文，便于故障排查与调用溯源。
//! - **代价与局限**：对部分底层三方库的偶发错误进行统一转换封装。

use thiserror::Error;
use waylan_algo::error::AlgoError;
use waylan_ibus::IbusError;
use waylan_wayland::WaylandRunnerError;

/// 微澜守护进程强类型错误枚举
#[derive(Debug, Error)]
pub enum DaemonError {
    /// 日志系统初始化失败
    #[error("日志系统初始化失败: {0}")]
    Logger(String),

    /// 底层文件或系统 I/O 错误
    #[error("系统 I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 算法词库与配置资源错误
    #[error("词库资源错误: {0}")]
    Algo(#[from] AlgoError),

    /// 自检或状态校验失败
    #[error("自检校验失败: {0}")]
    Validation(String),

    /// Wayland 会话运行期错误
    #[error("Wayland 会话运行错误: {0}")]
    Wayland(#[from] WaylandRunnerError),

    /// IBus 引擎会话运行期错误
    #[error("IBus 会话运行错误: {0}")]
    Ibus(#[from] IbusError),

    /// CLI 命令行参数解析或子命令执行错误
    #[error("命令行执行错误: {0}")]
    Cli(String),
}
