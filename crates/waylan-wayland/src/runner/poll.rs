//! Wayland 事件轮询、文件监控与驱动调度抽象
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1 与 V2 事件循环中高度重复的描述符多路复用 poll、
//!   唤醒通道排空、文件监控状态同步与超时计算样板代码。
//! - **核心优势**：通过 Trait 抽象提取统一的事件循环单步执行流 (`run_session_loop_step`)，使各版本
//!   会话专注差异化协议逻辑（如 V2 的按键长按连击驱动），同时保持编译期零抽象成本与安全的所有权控制。
//! - **代价与局限**：调用方需为各版本会话实现抽象方法（大多直接委托至内部 context 字段）。

use std::io::ErrorKind;
use std::sync::atomic::Ordering;

use rustix::event::{poll, PollFd, PollFlags};
use rustix::fd::BorrowedFd;
use rustix::io::Errno;
use wayland_client::backend::{ReadEventsGuard, WaylandError};
use wayland_client::EventQueue;

use crate::error::WaylandRunnerError;
use crate::session_v1::WaylanV1Session;
use crate::session_v2::WaylanV2Session;

/// 事件循环 poll 多路复用分发结果
pub(super) enum PollEventOutcome {
    /// 超时到达（poll 返回 0）
    TimedOut,
    /// 监测到就绪事件（唤醒或 inotify 描述符变化）
    WokenUp {
        wakeup_triggered: bool,
        inotify_triggered: bool,
    },
    /// 被系统中断信号唤醒
    Interrupted,
}

/// 解析 poll 就绪结果，返回唤醒通道与 inotify 监控各自的触发标记
fn interpret_poll_result(
    fds: &[PollFd<'_>],
    inotify_idx: Option<usize>,
    wakeup_idx: Option<usize>,
) -> (bool, bool) {
    let wakeup_triggered = wakeup_idx
        .map(|idx| {
            fds[idx]
                .revents()
                .intersects(PollFlags::IN | PollFlags::ERR)
        })
        .unwrap_or(false);
    let inotify_triggered = inotify_idx
        .map(|idx| {
            fds[idx]
                .revents()
                .intersects(PollFlags::IN | PollFlags::ERR)
        })
        .unwrap_or(false);

    (wakeup_triggered, inotify_triggered)
}

/// 在 poll 就绪后读取 Wayland 事件，致命 IO 错误向上抛出
///
/// 传入 `None` 表示本次无 Wayland 事件可读，读取守卫随参数析构立即释放读锁。
fn read_wayland_events(guard: Option<ReadEventsGuard>) -> Result<(), WaylandRunnerError> {
    let Some(guard) = guard else {
        return Ok(());
    };

    if let Err(WaylandError::Io(io_err)) = guard.read() {
        if io_err.kind() != ErrorKind::WouldBlock {
            return Err(io_err.into());
        }
    }
    Ok(())
}

/// 执行事件循环多路复用 poll 与 Wayland 消息读取
pub(super) fn poll_and_read_events(
    guard: ReadEventsGuard,
    timeout_ms: i32,
    inotify_fd: Option<BorrowedFd<'_>>,
    wakeup_fd: Option<BorrowedFd<'_>>,
) -> Result<PollEventOutcome, WaylandRunnerError> {
    let wayland_fd = guard.connection_fd();
    let mut fds = [
        PollFd::new(&wayland_fd, PollFlags::IN | PollFlags::ERR),
        PollFd::new(&wayland_fd, PollFlags::empty()),
        PollFd::new(&wayland_fd, PollFlags::empty()),
    ];
    let mut fd_count = 1;

    let inotify_idx = inotify_fd.as_ref().map(|fd| {
        let idx = fd_count;
        fds[idx] = PollFd::new(fd, PollFlags::IN | PollFlags::ERR);
        fd_count += 1;
        idx
    });

    let wakeup_idx = wakeup_fd.as_ref().map(|fd| {
        let idx = fd_count;
        fds[idx] = PollFd::new(fd, PollFlags::IN | PollFlags::ERR);
        fd_count += 1;
        idx
    });

    match poll(&mut fds[..fd_count], timeout_ms) {
        Ok(0) => {
            drop(guard);
            Ok(PollEventOutcome::TimedOut)
        }
        Ok(_) => {
            let (wakeup_triggered, inotify_triggered) =
                interpret_poll_result(&fds, inotify_idx, wakeup_idx);

            let wayland_ready = fds[0].revents().intersects(PollFlags::IN | PollFlags::ERR);
            read_wayland_events(if wayland_ready { Some(guard) } else { None })?;

            Ok(PollEventOutcome::WokenUp {
                wakeup_triggered,
                inotify_triggered,
            })
        }
        Err(Errno::INTR) => Ok(PollEventOutcome::Interrupted),
        Err(e) => Err(e.into()),
    }
}

use crate::session_common::WaylandSessionLifecycle;

/// Wayland 会话事件驱动调度抽象特型
pub(super) trait WaylandLoopSession: WaylandSessionLifecycle + Sized {
    /// 检查是否收到外部优雅退出请求
    fn is_exit_requested(&self) -> bool;

    /// 计算下一次 poll 的超时毫秒数
    fn compute_poll_timeout_ms(&self) -> i32;

    /// 获取 inotify 文件监控描述符（若已开启）
    fn inotify_fd(&self) -> Option<BorrowedFd<'_>>;

    /// 获取跨线程唤醒描述符（若已配置）
    fn wakeup_fd(&self) -> Option<BorrowedFd<'_>>;

    /// 排空并重置跨线程唤醒事件
    fn drain_wakeup(&mut self);

    /// 执行每次 poll 结束后的驱动钩子（如语音周期、按键连击）
    fn on_poll_completed(&mut self);

    /// 当前会话是否拥有活跃的 inotify 监听器
    fn has_inotify(&self) -> bool;
}

impl WaylandLoopSession for WaylanV1Session {
    #[inline]
    fn is_exit_requested(&self) -> bool {
        self.watcher
            .exit_requested
            .as_ref()
            .map(|f| f.load(Ordering::Acquire))
            .unwrap_or(false)
    }

    #[inline]
    fn compute_poll_timeout_ms(&self) -> i32 {
        let default_poll_timeout = if self.is_voice_active() {
            40
        } else if self.watcher.inotify_watcher.is_some() {
            -1
        } else {
            2000
        };
        self.next_poll_timeout_ms(default_poll_timeout)
    }

    #[inline]
    fn inotify_fd(&self) -> Option<BorrowedFd<'_>> {
        self.watcher.inotify_watcher.as_ref().map(|w| w.as_fd())
    }

    #[inline]
    fn wakeup_fd(&self) -> Option<BorrowedFd<'_>> {
        self.watcher.loop_wakeup.as_ref().map(|w| w.as_fd())
    }

    #[inline]
    fn drain_wakeup(&mut self) {
        if let Some(ref wakeup) = self.watcher.loop_wakeup {
            wakeup.drain();
        }
    }

    #[inline]
    fn on_poll_completed(&mut self) {
        self.poll_voice_cycle();
    }

    #[inline]
    fn has_inotify(&self) -> bool {
        self.watcher.inotify_watcher.is_some()
    }
}

impl WaylandLoopSession for WaylanV2Session {
    #[inline]
    fn is_exit_requested(&self) -> bool {
        self.watcher
            .exit_requested
            .as_ref()
            .map(|f| f.load(Ordering::Acquire))
            .unwrap_or(false)
    }

    #[inline]
    fn compute_poll_timeout_ms(&self) -> i32 {
        let default_poll_timeout = if self.is_voice_active() {
            40
        } else if self.watcher.inotify_watcher.is_some() {
            -1
        } else {
            2000
        };
        self.next_poll_timeout_ms(default_poll_timeout)
    }

    #[inline]
    fn inotify_fd(&self) -> Option<BorrowedFd<'_>> {
        self.watcher.inotify_watcher.as_ref().map(|w| w.as_fd())
    }

    #[inline]
    fn wakeup_fd(&self) -> Option<BorrowedFd<'_>> {
        self.watcher.loop_wakeup.as_ref().map(|w| w.as_fd())
    }

    #[inline]
    fn drain_wakeup(&mut self) {
        if let Some(ref wakeup) = self.watcher.loop_wakeup {
            wakeup.drain();
        }
    }

    #[inline]
    fn on_poll_completed(&mut self) {
        self.drive_key_repeat();
        self.poll_voice_cycle();
    }

    #[inline]
    fn has_inotify(&self) -> bool {
        self.watcher.inotify_watcher.is_some()
    }
}

/// 执行单步 Wayland 事件调度与多路复用 poll
///
/// # 返回值
/// - `Ok(true)`：单步轮询处理完成，继续下一轮事件循环。
/// - `Ok(false)`：检测到退出请求信号，安全退出事件循环。
/// - `Err(e)`：事件派发或底层 I/O 错误。
pub(super) fn run_session_loop_step<S: WaylandLoopSession>(
    event_queue: &mut EventQueue<S>,
    session: &mut S,
) -> Result<bool, WaylandRunnerError> {
    if session.is_exit_requested() {
        return Ok(false);
    }

    event_queue.dispatch_pending(session)?;
    event_queue.flush()?;

    let timeout_ms = session.compute_poll_timeout_ms();
    let inotify_fd = session.inotify_fd();
    let wakeup_fd = session.wakeup_fd();

    if let Some(guard) = event_queue.prepare_read() {
        let outcome = poll_and_read_events(guard, timeout_ms, inotify_fd, wakeup_fd)?;
        session.on_poll_completed();
        match outcome {
            PollEventOutcome::TimedOut => {
                if !session.has_inotify() {
                    session.sync_external_state();
                }
            }
            PollEventOutcome::WokenUp {
                wakeup_triggered,
                inotify_triggered,
            } => {
                if wakeup_triggered {
                    session.drain_wakeup();
                    session.sync_external_state();
                } else if inotify_triggered {
                    session.sync_external_state();
                }
            }
            PollEventOutcome::Interrupted => {}
        }
    }

    Ok(true)
}
