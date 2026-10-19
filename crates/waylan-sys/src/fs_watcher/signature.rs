//! 被监控文件集合的状态签名与基于时间戳/哈希的轮询降级检测
//!
//! # 设计原理
//! - **实现初衷**：在 inotify 不可用或处于降级模式时，提供基于 mtime 与复合哈希指纹的文件状态变更比对。
//! - **核心优势**：结合文件存在性、相对索引与纳秒修改时间戳生成 64 位防碰撞哈希，消除伪时间戳坏味道。

use std::path::PathBuf;
use std::time::SystemTime;

/// 被监控文件集合的状态签名（支持真实单文件时间戳与多文件复合哈希指纹，消除伪时间戳坏味道）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSignature {
    /// 单个文件的物理修改时间戳
    SingleFile(SystemTime),
    /// 多文件组合的状态哈希指纹（结合文件存在性、索引位置与修改时间戳）
    MultiFilesHash(u64),
}

/// 检查监控文件列表中是否有任何文件发生修改或被删除，并在检测到变更时更新基准签名（降级回退模式）
pub fn check_watched_paths_changed(
    watched_paths: &[PathBuf],
    last_signature: &mut Option<FileSignature>,
) -> bool {
    if watched_paths.is_empty() {
        return false;
    }

    let mut any_exists = false;
    let mut single_mtime = None;
    let mut combined_hash: u64 = 0xcbf29ce484222325;

    for (idx, p) in watched_paths.iter().enumerate() {
        if let Ok(meta) = std::fs::metadata(p) {
            if let Ok(mtime) = meta.modified() {
                any_exists = true;
                single_mtime = Some(mtime);
                let nanos = mtime
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                combined_hash ^= (idx as u64).wrapping_mul(0x100000001b3);
                combined_hash ^= nanos;
                combined_hash = combined_hash.wrapping_mul(0x100000001b3);
            }
        } else {
            combined_hash ^= (idx as u64).wrapping_add(0x517cc1b727220a95);
            combined_hash = combined_hash.wrapping_mul(0x100000001b3);
        }
    }

    let current_signature = if !any_exists {
        None
    } else if watched_paths.len() == 1 {
        single_mtime.map(FileSignature::SingleFile)
    } else {
        Some(FileSignature::MultiFilesHash(combined_hash))
    };

    match (*last_signature, current_signature) {
        (Some(old), Some(new)) if new != old => {
            *last_signature = Some(new);
            true
        }
        (Some(_), None) => {
            *last_signature = None;
            true
        }
        (None, Some(new)) => {
            *last_signature = Some(new);
            false
        }
        _ => false,
    }
}

/// 计算监控路径列表的初始状态签名
pub fn compute_initial_signature(watched_paths: &[PathBuf]) -> Option<FileSignature> {
    let mut sig = None;
    check_watched_paths_changed(watched_paths, &mut sig);
    sig
}
