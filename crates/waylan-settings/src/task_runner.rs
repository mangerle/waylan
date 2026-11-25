//! 设置中心统一后台任务调度器与生命周期纳管
//!
//! # 设计原理
//! - **实现初衷**：消除 `desktop`、`dict`、`tasks`、`cloud_profiles`、`model_download`、
//!   `model_import` 等模块中重复散落的 `std::thread::Builder::new()` 样板与孤儿线程隐患。
//! - **核心优势**：统一纳管所有后台工作线程的 `JoinHandle` 与全局取消标志 (`cancel_requested`)，
//!   在线程创建失败时自动回滚防重入标志，并在设置窗口关闭时统一通知取消与回收已完成句柄。
//! - **代价与局限**：长耗时阻塞系统调用（如第三方对话框进程或网络下载）若未轮询取消标志，
//!   窗口退出时仅通知取消并释放已完成线程句柄，不强行阻塞 GUI 退出。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};

use parking_lot::Mutex;

/// 设置中心后台任务调度器
pub struct BackgroundTaskRunner {
    cancel_requested: Arc<AtomicBool>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for BackgroundTaskRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundTaskRunner {
    /// 创建后台任务调度器
    pub fn new() -> Self {
        Self {
            cancel_requested: Arc::new(AtomicBool::new(false)),
            handles: Mutex::new(Vec::new()),
        }
    }

    /// 获取全局取消标志共享句柄（供长耗时后台任务定期检查是否应提前中止）
    #[inline]
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel_requested)
    }

    /// 查询是否已请求关闭全部后台任务
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_requested.load(Ordering::Acquire)
    }

    /// 派生并纳管一个具名后台线程
    ///
    /// # Errors
    /// 当操作系统线程资源不足导致派生失败时返回可读中文错误文本。
    pub fn spawn_named<F>(&self, thread_name: &str, worker: F) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        if self.is_cancelled() {
            return Err("设置中心正在退出，已拒绝派生新后台任务".to_string());
        }
        let handle = thread::Builder::new()
            .name(thread_name.to_string())
            .spawn(worker)
            .map_err(|e| e.to_string())?;

        let mut guard = self.handles.lock();
        guard.retain(|h| !h.is_finished());
        guard.push(handle);
        Ok(())
    }

    /// 派生带防重入原子标志的具名后台线程，派生失败时自动回滚标志并输出告警日志
    pub fn spawn_guarded<F>(
        &self,
        thread_name: &str,
        running_flag: &Arc<AtomicBool>,
        failure_hint: &str,
        worker: F,
    ) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        match self.spawn_named(thread_name, worker) {
            Ok(()) => true,
            Err(e) => {
                running_flag.store(false, Ordering::Release);
                log::warn!("[偏好设置 警告] {}：{}", failure_hint, e);
                false
            }
        }
    }

    /// 通知所有后台任务取消并回收已结束的线程句柄
    pub fn shutdown(&self) {
        self.cancel_requested.store(true, Ordering::Release);
        let mut guard = self.handles.lock();
        let drained = std::mem::take(&mut *guard);
        drop(guard);

        for handle in drained {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

/// 获取设置中心全局后台任务调度器单例
pub(crate) fn global_task_runner() -> &'static BackgroundTaskRunner {
    static RUNNER: OnceLock<BackgroundTaskRunner> = OnceLock::new();
    RUNNER.get_or_init(BackgroundTaskRunner::new)
}
