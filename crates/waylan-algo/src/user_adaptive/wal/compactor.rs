//! WAL 预写日志周期性归并与原子压缩落盘
//!
//! # 设计原理
//! - **实现初衷**：负责将内存模型中的全部去重聚合词条写入临时文件，并通过原子重命名替换原 WAL，
//!   清除冗余日志并回收磁盘空间。
//! - **核心优势**：
//!   1. 跨进程文件排他锁（flock）保护，彻底防止多实例同时压缩产生写冲突；
//!   2. RAII 模式临时文件守卫（TempFileGuard），任何环节异常自动清理垃圾文件；
//!   3. 原子重命名前后精确时间戳标记，避免外部监控事件误触发重复重载。

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::error::AlgoError;
use crate::user_adaptive::wal::constants::{WAL_CHECKSUM_SEED, WAL_MAGIC};
use crate::user_adaptive::wal::lock::ProcessFileLock;
use crate::user_adaptive::UserAdaptiveModel;

struct TempFileGuard<'a>(&'a Path, bool);

impl<'a> Drop for TempFileGuard<'a> {
    fn drop(&mut self) {
        if !self.1 {
            let _ = std::fs::remove_file(self.0);
        }
    }
}

/// 针对任意 UserAdaptiveModel 实例执行单次 WAL 归并压缩（无常驻句柄依赖）
pub fn compact_model<P: AsRef<Path>>(model: &UserAdaptiveModel, path: P) -> Result<(), AlgoError> {
    compact_model_with_callback(model, path.as_ref(), || {})
}

/// 执行 WAL 压缩落盘并回调标记时间戳（自动获取跨进程排他锁）
pub(crate) fn compact_model_with_callback<F: Fn()>(
    model: &UserAdaptiveModel,
    path_ref: &Path,
    mark_compact_fn: F,
) -> Result<(), AlgoError> {
    let _process_lock = ProcessFileLock::acquire_exclusive(path_ref).map_err(|e| {
        AlgoError::UserDictPersistFailed {
            path: path_ref.to_path_buf(),
            source: e,
        }
    })?;
    compact_model_internal(model, path_ref, mark_compact_fn)
}

/// 将模型去重词条按 WAL 二进制帧格式写入临时文件并同步落盘
fn write_compact_model_to_temp(
    model: &UserAdaptiveModel,
    temp_path: &Path,
) -> Result<(), AlgoError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options
        .open(temp_path)
        .map_err(|e| AlgoError::UserDictPersistFailed {
            path: temp_path.to_path_buf(),
            source: e,
        })?;

    file.write_all(WAL_MAGIC)
        .map_err(|e| AlgoError::UserDictPersistFailed {
            path: temp_path.to_path_buf(),
            source: e,
        })?;

    for list in model.entries.values() {
        for entry in list {
            let py_bytes = entry.pinyin.as_bytes();
            let text_bytes = entry.text.as_bytes();
            if py_bytes.len() > u16::MAX as usize || text_bytes.len() > u16::MAX as usize {
                continue;
            }
            let py_len = py_bytes.len() as u16;
            let text_len = text_bytes.len() as u16;

            let mut buf = Vec::with_capacity(20 + py_bytes.len() + text_bytes.len());
            buf.extend_from_slice(&entry.last_used_timestamp.to_le_bytes());
            buf.extend_from_slice(&entry.commit_count.to_le_bytes());
            buf.extend_from_slice(&py_len.to_le_bytes());
            buf.extend_from_slice(&text_len.to_le_bytes());
            buf.extend_from_slice(py_bytes);
            buf.extend_from_slice(text_bytes);
            let checksum: u32 = buf
                .iter()
                .fold(WAL_CHECKSUM_SEED, |acc, &b| acc.wrapping_add(b as u32));
            buf.extend_from_slice(&checksum.to_le_bytes());

            file.write_all(&buf)
                .map_err(|e| AlgoError::UserDictPersistFailed {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
        }
    }

    file.sync_all()
        .map_err(|e| AlgoError::UserDictPersistFailed {
            path: temp_path.to_path_buf(),
            source: e,
        })
}

/// 在已持有排他锁的上下文中执行核心 WAL 压缩落盘与原子重命名
pub(crate) fn compact_model_internal<F: Fn()>(
    model: &UserAdaptiveModel,
    path_ref: &Path,
    mark_compact_fn: F,
) -> Result<(), AlgoError> {
    if let Some(parent) = path_ref.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let pid = std::process::id();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_stem = path_ref
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("wal");
    let temp_file_name = format!("{}.{}.{}.tmp", file_stem, pid, timestamp);
    let temp_path = path_ref.with_file_name(temp_file_name);

    let mut guard = TempFileGuard(&temp_path, false);

    write_compact_model_to_temp(model, &temp_path)?;

    // 在原子重命名前即刻激活内部归并标记，防止内核派发重命名/删除事件与落盘标记产生竞态
    mark_compact_fn();

    std::fs::rename(&temp_path, path_ref).map_err(|e| AlgoError::UserDictPersistFailed {
        path: PathBuf::from(path_ref),
        source: e,
    })?;

    // 成功完成原子重命名后解除守卫删除并刷新内部落盘完成时间戳
    guard.1 = true;
    mark_compact_fn();

    Ok(())
}
