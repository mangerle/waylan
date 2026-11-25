//! 事件循环跨线程与信号唤醒机制模块
//!
//! 基于 Linux `eventfd` 提供纳秒级跨线程/信号事件循环唤醒通道，支持零开销事件驱动。

use log::warn;
use rustix::event::{EventfdFlags, eventfd};
use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::io::Errno;
use std::io;
use std::sync::Arc;

/// 事件循环跨线程/异步唤醒器
///
/// # 设计原理
/// - **实现初衷**：在纯事件驱动架构下，托盘线程与外部信号需要即时唤醒处于无限等待休眠中的 Wayland/IBus 事件循环。
/// - **核心优势**：基于 Linux 原生 eventfd 机制，具备纳秒级通知响应与零堆分配唤醒，支持多线程安全并发触发。
/// - **代价与局限**：依赖 Linux eventfd 系统调用，单次读写固定为 8 字节整型。
#[derive(Clone, Debug)]
pub struct LoopWakeup {
    fd: Arc<OwnedFd>,
}

impl LoopWakeup {
    /// 创建新的非阻塞事件唤醒器
    ///
    /// # Errors
    /// 当创建系统 eventfd 描述符失败时返回底层 I/O 错误。
    pub fn new() -> io::Result<Self> {
        let fd = eventfd(0, EventfdFlags::NONBLOCK | EventfdFlags::CLOEXEC)?;
        Ok(Self { fd: Arc::new(fd) })
    }

    /// 获取底层描述符的共享句柄
    pub fn fd(&self) -> Arc<OwnedFd> {
        Arc::clone(&self.fd)
    }

    /// 向事件循环发送唤醒脉冲（支持多线程安全并发调用）
    pub fn wake(&self) {
        let val = 1u64.to_ne_bytes();
        if let Err(e) = rustix::io::write(&*self.fd, &val) {
            // 当 eventfd 计数器已满（极罕见），也会保持处于可读就绪状态，不影响唤醒
            if e != Errno::WOULDBLOCK {
                warn!("[微澜事件驱动] 写入事件循环唤醒描述符失败: {}", e);
            }
        }
    }

    /// 供异步信号处理上下文调用的纯系统调用脉冲唤醒（无日志、无锁、零分配，严格 async-signal-safe）
    #[inline]
    pub fn wake_signal_safe(&self) {
        let val = 1u64.to_ne_bytes();
        let _ = rustix::io::write(&*self.fd, &val);
    }

    /// 清空并消耗当前挂起的全部唤醒事件计数
    pub fn drain(&self) {
        let mut buf = [0u8; 8];
        loop {
            match rustix::io::read(&*self.fd, &mut buf) {
                Ok(_) => {}
                Err(Errno::WOULDBLOCK) => break,
                Err(e) => {
                    warn!("[微澜事件驱动] 读取事件循环唤醒描述符异常: {}", e);
                    break;
                }
            }
        }
    }

    /// 获取底层文件描述符借用（供 poll 多路复用查询）
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl AsFd for LoopWakeup {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loop_wakeup_pulse_and_drain() {
        let wakeup = LoopWakeup::new().expect("创建 LoopWakeup 应当成功");
        wakeup.wake();
        wakeup.wake();
        wakeup.drain();
        wakeup.drain();
    }
}
