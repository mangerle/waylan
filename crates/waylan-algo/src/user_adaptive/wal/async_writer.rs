//! 用户词库 WAL 异步后台落盘调度器
//!
//! # 设计原理
//! - **实现初衷**：消除在输入法按键/选词热路径（如 IBus D-Bus 异步执行器或 Wayland 主循环）上
//!   直接调用跨进程文件锁 (`flock`) 与磁盘写入造成的微秒至毫秒级卡顿。
//! - **核心优势**：主线程在选词/忘词时仅执行纯内存模型更新，并通过有界同步通道 (`sync_channel`)
//!   将增量 WAL 帧投递给专用后台线程；`compact()` 则通过确认通道同步等待在途帧全部落盘并归并。
//! - **代价与局限**：极端情况下若后台队列积压达到容量上限（256 条），`try_send` 会丢弃本次增量持久化并告警，
//!   但纯内存模型与后续 `compact()` 仍保持完整准确，绝不阻塞打字。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use log::{error, warn};
use parking_lot::Mutex;

use crate::error::AlgoError;
use crate::user_adaptive::UserAdaptiveModel;
use crate::user_adaptive::wal::{
    UserAdaptiveWal, UserDictAppendParams, UserDictForgetParams, UserDictPairParams,
};

/// 异步 WAL 通道容量上限（严禁无界通道，防止磁盘极端卡顿时内存无限积压）
const ASYNC_WAL_QUEUE_CAPACITY: usize = 256;

/// 当前后台线程已打开的 WAL 物理文件标识（用于无阻塞判定 `is_same_wal_file`）
#[derive(Debug, Clone)]
struct OpenedFileIdentity {
    path: PathBuf,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    len: u64,
}

/// 异步 WAL 后台线程指令集
enum WalAsyncCommand {
    /// 追加单条选词上屏帧
    Append {
        path: PathBuf,
        text: String,
        pinyin: String,
        timestamp: u64,
    },
    /// 追加单条词对搭配帧
    AppendPair {
        path: PathBuf,
        prev: String,
        curr: String,
        timestamp: u64,
    },
    /// 追加单条精准忘词帧
    AppendForget {
        path: PathBuf,
        text: String,
        pinyin: Option<String>,
        timestamp: u64,
    },
    /// 条件压缩（仅在后台累计追加次数达到阈值时执行归并）
    MaybeCompact {
        path: PathBuf,
        snapshot: Box<UserAdaptiveModel>,
    },
    /// 强制同步归并压缩，并通过确认通道通知调用方
    CompactSync {
        path: PathBuf,
        snapshot: Box<UserAdaptiveModel>,
        ack_tx: SyncSender<Result<(), AlgoError>>,
    },
    /// 刷盘并安全退出后台线程
    Shutdown,
}

/// 用户词库 WAL 异步后台写入器
#[derive(Debug)]
pub struct AsyncWalWriter {
    cmd_tx: Option<SyncSender<WalAsyncCommand>>,
    worker: Option<JoinHandle<()>>,
    last_compact_ms: Arc<AtomicU64>,
    pending_appends: Arc<AtomicUsize>,
    opened_identity: Arc<Mutex<Option<OpenedFileIdentity>>>,
}

impl Default for AsyncWalWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncWalWriter {
    /// 创建异步 WAL 写入器并派生后台落盘线程
    pub fn new() -> Self {
        let wal = UserAdaptiveWal::new();
        let last_compact_ms = wal.last_compact_time();
        let pending_appends = Arc::new(AtomicUsize::new(0));
        let opened_identity = Arc::new(Mutex::new(None));

        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<WalAsyncCommand>(ASYNC_WAL_QUEUE_CAPACITY);

        let pending_clone = Arc::clone(&pending_appends);
        let identity_clone = Arc::clone(&opened_identity);

        let worker = match thread::Builder::new()
            .name("waylan-wal-writer".to_string())
            .spawn(move || {
                run_wal_worker(wal, cmd_rx, pending_clone, identity_clone);
            }) {
            Ok(handle) => Some(handle),
            Err(e) => {
                error!(
                    "[用户词库 WAL] 派生后台落盘线程失败，将降级为仅内存运行: {}",
                    e
                );
                None
            }
        };

        Self {
            cmd_tx: worker.as_ref().map(|_| cmd_tx),
            worker,
            last_compact_ms,
            pending_appends,
            opened_identity,
        }
    }

    /// 异步投递单条选词上屏记录
    pub fn send_append(&self, path: &Path, text: &str, pinyin: &str, timestamp: u64) {
        let Some(ref tx) = self.cmd_tx else {
            return;
        };
        if let Err(e) = tx.try_send(WalAsyncCommand::Append {
            path: path.to_path_buf(),
            text: text.to_string(),
            pinyin: pinyin.to_string(),
            timestamp,
        }) {
            warn!(
                "[用户词库 WAL] 异步落盘队列已满或已关闭，跳过本次上屏记录写盘: {}",
                e
            );
        }
    }

    /// 异步投递单条词对搭配记录
    pub fn send_append_pair(&self, path: &Path, prev: &str, curr: &str, timestamp: u64) {
        let Some(ref tx) = self.cmd_tx else {
            return;
        };
        if let Err(e) = tx.try_send(WalAsyncCommand::AppendPair {
            path: path.to_path_buf(),
            prev: prev.to_string(),
            curr: curr.to_string(),
            timestamp,
        }) {
            warn!(
                "[用户词库 WAL] 异步落盘队列已满或已关闭，跳过本次词对记录写盘: {}",
                e
            );
        }
    }

    /// 异步投递单条忘词记录
    pub fn send_append_forget(
        &self,
        path: &Path,
        text: &str,
        pinyin: Option<&str>,
        timestamp: u64,
    ) {
        let Some(ref tx) = self.cmd_tx else {
            return;
        };
        if let Err(e) = tx.try_send(WalAsyncCommand::AppendForget {
            path: path.to_path_buf(),
            text: text.to_string(),
            pinyin: pinyin.map(str::to_string),
            timestamp,
        }) {
            warn!(
                "[用户词库 WAL] 异步落盘队列已满或已关闭，跳过本次忘词记录写盘: {}",
                e
            );
        }
    }

    /// 检查是否达到压缩阈值，若达标则异步投递归并任务（未达标时零克隆返回 `false`）
    pub fn maybe_compact(&self, path: &Path, model: &UserAdaptiveModel) -> bool {
        if self.pending_appends.load(Ordering::Acquire) < UserAdaptiveWal::DEFAULT_COMPACT_THRESHOLD
        {
            return false;
        }
        let Some(ref tx) = self.cmd_tx else {
            return false;
        };
        let snapshot = Box::new(model.clone());
        match tx.try_send(WalAsyncCommand::MaybeCompact {
            path: path.to_path_buf(),
            snapshot,
        }) {
            Ok(()) => true,
            Err(e) => {
                warn!("[用户词库 WAL] 投递异步归并任务失败: {}", e);
                false
            }
        }
    }

    /// 同步执行归并压缩并等待后台在途写入全部完成
    ///
    /// # Errors
    /// 当后台压缩落盘失败时返回对应的 `AlgoError`。
    pub fn compact_sync(&self, path: &Path, model: &UserAdaptiveModel) -> Result<(), AlgoError> {
        let Some(ref tx) = self.cmd_tx else {
            return UserAdaptiveWal::compact_model(model, path);
        };
        let (ack_tx, ack_rx) = mpsc::sync_channel(1);
        let cmd = WalAsyncCommand::CompactSync {
            path: path.to_path_buf(),
            snapshot: Box::new(model.clone()),
            ack_tx,
        };
        if tx.send(cmd).is_err() {
            return UserAdaptiveWal::compact_model(model, path);
        }
        ack_rx
            .recv()
            .unwrap_or_else(|_| UserAdaptiveWal::compact_model(model, path))
    }

    /// 获取最近一次内部 WAL 归并落盘时间戳共享原子句柄
    #[inline]
    pub fn last_compact_time(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.last_compact_ms)
    }

    /// 判断磁盘上的 WAL 文件是否与后台当前持有的常驻句柄指向同一文件
    pub fn is_same_wal_file(&self, path: &Path) -> bool {
        let guard = self.opened_identity.lock();
        let Some(ref id) = *guard else {
            return false;
        };
        if id.path != path {
            return false;
        }
        let Ok(disk_meta) = std::fs::metadata(path) else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            id.dev == disk_meta.dev() && id.ino == disk_meta.ino() && id.len == disk_meta.len()
        }
        #[cfg(not(unix))]
        {
            id.len == disk_meta.len()
        }
    }
}

impl Drop for AsyncWalWriter {
    fn drop(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(WalAsyncCommand::Shutdown);
        }
        if let Some(handle) = self.worker.take()
            && let Err(e) = handle.join()
        {
            warn!("[用户词库 WAL] 等待后台落盘线程退出异常: {:?}", e);
        }
    }
}

/// 刷新共享的物理文件标识快照
fn sync_identity(wal: &UserAdaptiveWal, target: &Arc<Mutex<Option<OpenedFileIdentity>>>) {
    let id = wal.current_opened_identity().map(|(path, dev, ino, len)| {
        let _ = (dev, ino);
        OpenedFileIdentity {
            path,
            #[cfg(unix)]
            dev,
            #[cfg(unix)]
            ino,
            len,
        }
    });
    *target.lock() = id;
}

/// 后台 WAL 落盘线程主循环
fn run_wal_worker(
    mut wal: UserAdaptiveWal,
    cmd_rx: Receiver<WalAsyncCommand>,
    pending_appends: Arc<AtomicUsize>,
    opened_identity: Arc<Mutex<Option<OpenedFileIdentity>>>,
) {
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            WalAsyncCommand::Append {
                path,
                text,
                pinyin,
                timestamp,
            } => {
                if let Err(e) = wal.append_frame_only(&UserDictAppendParams {
                    path: &path,
                    text: &text,
                    pinyin: &pinyin,
                    timestamp,
                }) {
                    warn!("[用户词库 WAL] 后台追加上屏记录失败: {}", e);
                } else {
                    pending_appends.store(wal.wal_append_counter(), Ordering::Release);
                    sync_identity(&wal, &opened_identity);
                }
            }
            WalAsyncCommand::AppendPair {
                path,
                prev,
                curr,
                timestamp,
            } => {
                if let Err(e) = wal.append_pair_frame_only(&UserDictPairParams {
                    path: &path,
                    prev: &prev,
                    curr: &curr,
                    timestamp,
                }) {
                    warn!("[用户词库 WAL] 后台追加词对记录失败: {}", e);
                } else {
                    pending_appends.store(wal.wal_append_counter(), Ordering::Release);
                    sync_identity(&wal, &opened_identity);
                }
            }
            WalAsyncCommand::AppendForget {
                path,
                text,
                pinyin,
                timestamp,
            } => {
                if let Err(e) = wal.append_forget_frame_only(&UserDictForgetParams {
                    path: &path,
                    text: &text,
                    pinyin: pinyin.as_deref(),
                    timestamp,
                }) {
                    warn!("[用户词库 WAL] 后台追加忘词记录失败: {}", e);
                } else {
                    pending_appends.store(wal.wal_append_counter(), Ordering::Release);
                    sync_identity(&wal, &opened_identity);
                }
            }
            WalAsyncCommand::MaybeCompact { path, snapshot } => {
                match wal.maybe_compact(&snapshot, &path) {
                    Ok(true) => {
                        pending_appends.store(0, Ordering::Release);
                        sync_identity(&wal, &opened_identity);
                    }
                    Ok(false) => {}
                    Err(e) => warn!("[用户词库 WAL] 后台自动归并失败: {}", e),
                }
            }
            WalAsyncCommand::CompactSync {
                path,
                snapshot,
                ack_tx,
            } => {
                let res = wal.compact(&snapshot, &path);
                if res.is_ok() {
                    wal.reset_append_counter();
                    pending_appends.store(0, Ordering::Release);
                    sync_identity(&wal, &opened_identity);
                }
                let _ = ack_tx.send(res);
            }
            WalAsyncCommand::Shutdown => break,
        }
    }
    wal.close_handle();
    *opened_identity.lock() = None;
}
