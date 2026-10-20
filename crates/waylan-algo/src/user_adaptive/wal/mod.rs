//! 用户词库预写日志（WAL）持久化模块
//!
//! # 架构优势
//! 1. 采用追加写（Append-Only），每次选词仅产生数十字节文件尾部写入，耗时控制在微秒级；
//! 2. 彻底根除每次选词保存全量序列化造成的磁盘 I/O 停顿；
//! 3. 管理常驻文件追加写入句柄，消除高频选词时的重复 open/close 系统调用开销；
//! 4. 彻底解耦领域纯内存模型与底层物理文件描述符的生命周期管理。

pub mod compactor;
pub mod constants;
pub mod lock;
pub mod replay;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use log::warn;
use parking_lot::Mutex;

use crate::error::AlgoError;
use crate::user_adaptive::UserAdaptiveModel;
use lock::ProcessFileLock;

pub use compactor::compact_model;
pub use constants::{WAL_CHECKSUM_SEED, WAL_MAGIC, WAL_MAGIC_V1};
pub use replay::load_or_replay;

/// 单条用户上屏记录的 WAL 追加参数
///
/// # 设计原理
/// - **实现初衷**：上屏记录追加需同时携带"日志路径、文本、拼音键与时间戳"四项同源信息，
///   平铺传参易发生文本与拼音键顺序颠倒，导致写入不可解码的 WAL 帧。
/// - **核心优势**：以参数对象聚合同源信息，调用点按名传参即可完成语义绑定；
///   同时去除泛型路径参数，统一以 `&Path` 传入，杜绝类型推断歧义。
/// - **代价与局限**：调用点需显式书写字段名。
#[derive(Debug, Clone, Copy)]
pub struct UserDictAppendParams<'a> {
    /// WAL 日志文件路径
    pub path: &'a Path,
    /// 上屏文本
    pub text: &'a str,
    /// 上屏文本对应的拼音键
    pub pinyin: &'a str,
    /// 上屏时间戳（毫秒级 UNIX 时间）
    pub timestamp: u64,
}

/// 用户词库预写日志（WAL）与磁盘文件持久化管理器
#[derive(Debug)]
pub struct UserAdaptiveWal {
    /// 常驻保持的 WAL 日志追加写入文件句柄（消除每次选词时的重复 open/stat/close 系统调用）
    wal_file: Mutex<Option<(PathBuf, File)>>,
    /// 运行期向 WAL 追加记录的计数器（用于触发周期性自动压缩）
    wal_append_counter: usize,
    /// 触发自动压缩的追加条数阈值（默认为 1000 条）
    compact_threshold: usize,
    /// 最近一次内部 WAL 归并压缩落盘完成的时间戳（毫秒级 UNIX 时间）
    last_compact_ms: Arc<AtomicU64>,
}

impl Default for UserAdaptiveWal {
    fn default() -> Self {
        Self::new()
    }
}

impl UserAdaptiveWal {
    /// 默认运行时触发自动压缩的追加阈值
    pub const DEFAULT_COMPACT_THRESHOLD: usize = 1000;

    /// 创建新的 WAL 持久化管理器
    pub fn new() -> Self {
        Self {
            wal_file: Mutex::new(None),
            wal_append_counter: 0,
            compact_threshold: Self::DEFAULT_COMPACT_THRESHOLD,
            last_compact_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 获取当前管理器绑定的归并落盘时间戳共享原子句柄
    pub fn last_compact_time(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.last_compact_ms)
    }

    /// 查询当前是否处于刚刚完成 WAL 归并落盘的防重保护窗口内（毫秒）
    pub fn is_recent_compact(&self, window_ms: u64) -> bool {
        let last = self.last_compact_ms.load(Ordering::Acquire);
        if last == 0 {
            return false;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        now.saturating_sub(last) <= window_ms
    }

    /// 标记当前完成内部 WAL 归并压缩落盘
    pub fn mark_internal_compact(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_compact_ms.store(now, Ordering::Release);
    }

    /// 设置运行期触发自动压缩的 WAL 追加流水阈值
    pub fn set_compact_threshold(&mut self, threshold: usize) {
        self.compact_threshold = threshold;
    }

    /// 获取当前未压缩的 WAL 追加流水计数值
    pub fn wal_append_counter(&self) -> usize {
        self.wal_append_counter
    }

    /// 重置 WAL 追加流水计数值
    pub fn reset_append_counter(&mut self) {
        self.wal_append_counter = 0;
    }

    /// 判断磁盘上的 WAL 文件是否与当前管理器持有的常驻追加句柄指向同一底层文件（相同 inode 且长度一致）
    pub fn is_same_wal_file(&self, path: &Path) -> bool {
        let guard = self.wal_file.lock();
        let Some((ref opened_path, ref opened_file)) = *guard else {
            return false;
        };
        if opened_path != path {
            return false;
        }
        let (Ok(opened_meta), Ok(disk_meta)) = (opened_file.metadata(), std::fs::metadata(path))
        else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            opened_meta.dev() == disk_meta.dev()
                && opened_meta.ino() == disk_meta.ino()
                && opened_meta.len() == disk_meta.len()
        }
        #[cfg(not(unix))]
        {
            opened_meta.len() == disk_meta.len()
        }
    }

    /// 判断已打开的文件是否与目标磁盘文件一致，需要重新打开
    fn check_need_reopen(current_opt: Option<&(PathBuf, File)>, target_path: &Path) -> bool {
        match current_opt {
            Some((current_path, opened_file)) => {
                if current_path != target_path {
                    true
                } else {
                    match (opened_file.metadata(), std::fs::metadata(target_path)) {
                        (Ok(opened_meta), Ok(disk_meta)) => {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::MetadataExt;
                                opened_meta.dev() != disk_meta.dev()
                                    || opened_meta.ino() != disk_meta.ino()
                            }
                            #[cfg(not(unix))]
                            {
                                false
                            }
                        }
                        _ => true,
                    }
                }
            }
            None => true,
        }
    }

    /// 校验并初始化打开 WAL 追加写文件句柄（支持旧格式升级与跨进程排他锁）
    fn ensure_wal_file_opened(
        &mut self,
        model: &mut UserAdaptiveModel,
        path_ref: &Path,
    ) -> Result<(), AlgoError> {
        {
            let guard = self.wal_file.lock();
            if !Self::check_need_reopen(guard.as_ref(), path_ref) {
                return Ok(());
            }
        }

        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        // 若目标文件已存在但并非当前 V2 WAL 格式，先合并重放旧数据再归并升级为标准 V2 WAL 格式
        if Self::is_legacy_wal_format(path_ref) {
            self.upgrade_legacy_wal(model, path_ref)?;
        }

        let mut guard = self.wal_file.lock();
        let file = Self::open_wal_append_file(path_ref)?;

        *guard = Some((PathBuf::from(path_ref), file));
        Ok(())
    }

    /// 判断目标路径上的 WAL 文件是否为待升级的旧格式（非空且不带 V2 魔数头）
    fn is_legacy_wal_format(path_ref: &Path) -> bool {
        if !path_ref.exists() {
            return false;
        }
        let Ok(mut existing_file) = File::open(path_ref) else {
            return false;
        };
        let len = existing_file.metadata().map(|m| m.len()).unwrap_or(0);
        if len == 0 {
            return false;
        }
        let mut magic = [0u8; 16];
        existing_file.read_exact(&mut magic).is_err() || &magic != WAL_MAGIC
    }

    /// 将旧格式 WAL 中的历史词条合并入内存模型并归并升级为标准 V2 WAL
    fn upgrade_legacy_wal(
        &self,
        model: &mut UserAdaptiveModel,
        path_ref: &Path,
    ) -> Result<(), AlgoError> {
        if let Ok(old_model) = replay::load_or_replay_without_lock(path_ref) {
            model.merge(&old_model);
        }
        compactor::compact_model_internal(model, path_ref, || {
            self.mark_internal_compact();
        })
    }

    /// 以追加模式打开 WAL 日志文件并按需写入 V2 魔数头（受跨进程排他锁保护）
    fn open_wal_append_file(path_ref: &Path) -> Result<File, AlgoError> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);

        let mut file = options
            .open(path_ref)
            .map_err(|e| AlgoError::UserDictPersistFailed {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        // 文件大小为 0 时写入魔数头（受跨进程排他锁保护）
        if file.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
            file.write_all(WAL_MAGIC)
                .map_err(|e| AlgoError::UserDictPersistFailed {
                    path: PathBuf::from(path_ref),
                    source: e,
                })?;
        }

        Ok(file)
    }

    /// 序列化单条用户上屏 WAL 帧二进制数据包（包含校验和）
    fn serialize_wal_frame(text: &str, pinyin: &str, timestamp: u64) -> Option<Vec<u8>> {
        let py_bytes = pinyin.as_bytes();
        let text_bytes = text.as_bytes();
        if py_bytes.len() > u16::MAX as usize || text_bytes.len() > u16::MAX as usize {
            warn!(
                "待写入的用户词条长度超过 65535 字节上限，跳过写入: py_len={}, text_len={}",
                py_bytes.len(),
                text_bytes.len()
            );
            return None;
        }

        let py_len = py_bytes.len() as u16;
        let text_len = text_bytes.len() as u16;
        let count: u32 = 1;

        let mut buf = Vec::with_capacity(20 + py_bytes.len() + text_bytes.len());
        buf.extend_from_slice(&timestamp.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&py_len.to_le_bytes());
        buf.extend_from_slice(&text_len.to_le_bytes());
        buf.extend_from_slice(py_bytes);
        buf.extend_from_slice(text_bytes);

        // 简易校验和防部分写入损坏（带非零初始种子，杜绝全零块绕过）
        let checksum: u32 = buf
            .iter()
            .fold(WAL_CHECKSUM_SEED, |acc, &b| acc.wrapping_add(b as u32));
        buf.extend_from_slice(&checksum.to_le_bytes());

        Some(buf)
    }

    /// 追加单条用户上屏记录到 WAL 中，并同步更新传入的内存领域模型
    pub fn append(
        &mut self,
        model: &mut UserAdaptiveModel,
        params: &UserDictAppendParams<'_>,
    ) -> Result<(), AlgoError> {
        let path_ref = params.path;
        let frame_buf =
            match Self::serialize_wal_frame(params.text, params.pinyin, params.timestamp) {
                Some(buf) => buf,
                None => return Ok(()),
            };

        // 跨进程排他锁保护：从检查打开文件到写入单条帧均受排他互斥保护
        let _process_lock = ProcessFileLock::acquire_exclusive(path_ref).map_err(|e| {
            AlgoError::UserDictPersistFailed {
                path: PathBuf::from(path_ref),
                source: e,
            }
        })?;

        self.ensure_wal_file_opened(model, path_ref)?;

        let mut guard = self.wal_file.lock();

        if let Some((_, ref mut file)) = *guard {
            file.write_all(&frame_buf)
                .map_err(|e| AlgoError::UserDictPersistFailed {
                    path: PathBuf::from(path_ref),
                    source: e,
                })?;
        }

        drop(guard);

        // 同步更新内存模型状态机（遵循 WAL 先写日志后更新内存的设计原则）
        model.record_commit(params.text, params.pinyin, params.timestamp);

        // 运行期累计写入流水计数（解耦打字按键热路径与重度磁盘 IO，由外部在失焦或退出时调用 maybe_compact）
        self.wal_append_counter += 1;

        Ok(())
    }

    /// 检查并在达到压缩阈值时执行 WAL 文件的原子压缩归并 (Compaction)
    pub fn maybe_compact<P: AsRef<Path>>(
        &mut self,
        model: &UserAdaptiveModel,
        path: P,
    ) -> Result<bool, AlgoError> {
        if self.wal_append_counter >= self.compact_threshold {
            self.compact(model, path)?;
            self.wal_append_counter = 0;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 从 WAL 日志文件中重放还原用户词库模型，向下兼容旧版全量文件与旧版 V1 WAL
    pub fn load_or_replay<P: AsRef<Path>>(path: P) -> Result<UserAdaptiveModel, AlgoError> {
        replay::load_or_replay(path)
    }

    /// 对 WAL 文件执行压缩归并快照，清除冗余流水记录
    pub fn compact<P: AsRef<Path>>(
        &self,
        model: &UserAdaptiveModel,
        path: P,
    ) -> Result<(), AlgoError> {
        let path_ref = path.as_ref();
        // 压缩前主动释放常驻写入句柄，避免占用文件妨碍原子 rename
        *self.wal_file.lock() = None;

        compactor::compact_model_with_callback(model, path_ref, || {
            self.mark_internal_compact();
        })
    }

    /// 针对任意 UserAdaptiveModel 实例执行单次 WAL 归并压缩（无常驻句柄依赖）
    pub fn compact_model<P: AsRef<Path>>(
        model: &UserAdaptiveModel,
        path: P,
    ) -> Result<(), AlgoError> {
        compactor::compact_model(model, path)
    }
}
