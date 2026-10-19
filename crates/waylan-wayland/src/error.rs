//! Wayland 会话与事件循环错误类型定义

use thiserror::Error;

/// Wayland 事件循环运行错误枚举
#[derive(Debug, Error)]
pub enum WaylandRunnerError {
    /// 连接 Wayland 显示服务器失败
    #[error("连接 Wayland 显示服务器失败，原因: {reason}")]
    Connect {
        /// 具体失败原因
        reason: String,
    },

    /// 注册表初始化失败
    #[error("初始化 Wayland 全局注册表失败，原因: {reason}")]
    Registry {
        /// 具体失败原因
        reason: String,
    },

    /// 绑定全局协议对象失败
    #[error("绑定全局协议对象失败，原因: {reason}")]
    Bind {
        /// 具体失败原因
        reason: String,
    },

    /// 底层 Wayland 协议或传输错误
    #[error("Wayland 底层传输错误，原因: {reason}")]
    Wayland {
        /// 具体失败原因
        reason: String,
    },

    /// 绑定必需的全局协议对象失败
    #[error("绑定必需的 Wayland 全局协议失败，接口: {interface}，原因: {reason}")]
    MissingGlobal {
        /// 协议接口名称
        interface: &'static str,
        /// 具体失败原因
        reason: String,
    },

    /// 合成器缺少特定输入法扩展协议
    #[error("当前 Wayland 合成器缺少 {interface} 扩展协议，原因: {reason}")]
    UnsupportedProtocol {
        /// 扩展接口名称
        interface: &'static str,
        /// 具体失败原因
        reason: String,
    },

    /// Wayland 事件队列分发失败
    #[error("Wayland 事件队列派发失败，原因: {reason}")]
    Dispatch {
        /// 具体失败原因
        reason: String,
    },

    /// 描述符轮询或唤醒 I/O 错误
    #[error("Wayland 底层 I/O 错误，原因: {0}")]
    Io(#[from] std::io::Error),

    /// Linux 系统调用 Errno 错误
    #[error("系统调用执行失败，系统错误码: {errno}，详情: {message}")]
    Syscall {
        /// 操作系统错误码
        errno: i32,
        /// 错误描述
        message: String,
    },
}

impl From<wayland_client::ConnectError> for WaylandRunnerError {
    fn from(err: wayland_client::ConnectError) -> Self {
        Self::Connect {
            reason: err.to_string(),
        }
    }
}

impl From<wayland_client::globals::GlobalError> for WaylandRunnerError {
    fn from(err: wayland_client::globals::GlobalError) -> Self {
        Self::Registry {
            reason: err.to_string(),
        }
    }
}

impl From<wayland_client::globals::BindError> for WaylandRunnerError {
    fn from(err: wayland_client::globals::BindError) -> Self {
        Self::Bind {
            reason: err.to_string(),
        }
    }
}

impl From<wayland_client::backend::WaylandError> for WaylandRunnerError {
    fn from(err: wayland_client::backend::WaylandError) -> Self {
        Self::Wayland {
            reason: err.to_string(),
        }
    }
}

impl From<wayland_client::DispatchError> for WaylandRunnerError {
    fn from(err: wayland_client::DispatchError) -> Self {
        Self::Dispatch {
            reason: err.to_string(),
        }
    }
}

impl From<rustix::io::Errno> for WaylandRunnerError {
    fn from(err: rustix::io::Errno) -> Self {
        Self::Syscall {
            errno: err.raw_os_error(),
            message: err.to_string(),
        }
    }
}
