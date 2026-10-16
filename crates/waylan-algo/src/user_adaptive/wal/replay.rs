//! WAL 预写日志数据重放与反序列化
//!
//! # 设计原理
//! - **实现初衷**：负责从磁盘日志流中按帧解码记录，还原内存用户词库模型；
//!   同时自动检测损坏截断并安全熔断，兼容旧版全量文件与 V1 格式。
//! - **核心优势**：通过非零种子校验和防止脏写，遇到异常尾部帧优雅降级并触发自动压缩。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use log::warn;

use crate::error::AlgoError;
use crate::user_adaptive::wal::compactor::compact_model;
use crate::user_adaptive::wal::constants::{WAL_CHECKSUM_SEED, WAL_MAGIC, WAL_MAGIC_V1};
use crate::user_adaptive::UserAdaptiveModel;

/// 从二进制缓冲区重放 WAL 记录帧（公共重放逻辑）
pub(crate) fn replay_wal_records(buf: &[u8], is_v2: bool) -> (UserAdaptiveModel, bool, usize) {
    let mut model = UserAdaptiveModel::new();
    let mut record_count = 0;
    let mut cursor = 0;
    let mut has_corrupted_tail = false;
    let header_len = if is_v2 { 16 } else { 12 };

    while cursor + header_len <= buf.len() {
        let record_start = cursor;
        let timestamp = u64::from_le_bytes(buf[cursor..cursor + 8].try_into().unwrap_or([0; 8]));
        let count = if is_v2 {
            u32::from_le_bytes(buf[cursor + 8..cursor + 12].try_into().unwrap_or([0; 4]))
        } else {
            1
        };
        let (py_len, text_len) = if is_v2 {
            let p = u16::from_le_bytes(buf[cursor + 12..cursor + 14].try_into().unwrap_or([0; 2]))
                as usize;
            let t = u16::from_le_bytes(buf[cursor + 14..cursor + 16].try_into().unwrap_or([0; 2]))
                as usize;
            (p, t)
        } else {
            let p = u16::from_le_bytes(buf[cursor + 8..cursor + 10].try_into().unwrap_or([0; 2]))
                as usize;
            let t = u16::from_le_bytes(buf[cursor + 10..cursor + 12].try_into().unwrap_or([0; 2]))
                as usize;
            (p, t)
        };
        cursor += header_len;

        if cursor + py_len + text_len + 4 > buf.len() {
            warn!("[用户词库 WAL] 检测到文件末尾记录截断损坏，安全截断并在后续自动压缩");
            has_corrupted_tail = true;
            break;
        }

        let py_bytes = &buf[cursor..cursor + py_len];
        cursor += py_len;
        let text_bytes = &buf[cursor..cursor + text_len];
        cursor += text_len;

        let expected_checksum =
            u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap_or([0; 4]));
        cursor += 4;

        let payload = &buf[record_start..cursor - 4];
        let actual_checksum: u32 = payload
            .iter()
            .fold(WAL_CHECKSUM_SEED, |acc, &b| acc.wrapping_add(b as u32));
        if actual_checksum != expected_checksum {
            warn!(
                "[用户词库 WAL] 记录校验和不匹配 (预期: {:#x}, 实际: {:#x})，终止后续失步帧解析并执行归并清理",
                expected_checksum, actual_checksum
            );
            has_corrupted_tail = true;
            break;
        }

        let py_str = std::str::from_utf8(py_bytes).unwrap_or("");
        let text_str = std::str::from_utf8(text_bytes).unwrap_or("");

        if !py_str.is_empty() && !text_str.is_empty() {
            model.record_commit_count(text_str, py_str, timestamp, count);
            record_count += 1;
        }
    }

    (model, has_corrupted_tail, record_count)
}

/// 从 WAL 日志文件中重放还原用户词库模型，向下兼容旧版全量文件与旧版 V1 WAL
pub fn load_or_replay<P: AsRef<Path>>(path: P) -> Result<UserAdaptiveModel, AlgoError> {
    let path_ref = path.as_ref();
    if !path_ref.exists() {
        return Ok(UserAdaptiveModel::new());
    }

    let mut file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
        path: PathBuf::from(path_ref),
        source: e,
    })?;

    let meta = file.metadata().map_err(|e| AlgoError::DictionaryIo {
        path: PathBuf::from(path_ref),
        source: e,
    })?;
    if meta.len() > crate::user_adaptive::MAX_USER_DICT_BYTES {
        return Err(AlgoError::deserialize_failed(
            path_ref,
            format!(
                "WAL 用户词库文件体积异常 ({} 字节)，超出 64MB 上限",
                meta.len()
            ),
        ));
    }

    let mut magic = [0u8; 16];
    if file.read_exact(&mut magic).is_ok() {
        let is_v2 = &magic == WAL_MAGIC;
        let is_v1 = &magic == WAL_MAGIC_V1;
        if is_v2 || is_v1 {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)
                .map_err(|e| AlgoError::DictionaryIo {
                    path: PathBuf::from(path_ref),
                    source: e,
                })?;

            let (model, has_corrupted_tail, record_count) = replay_wal_records(&buf, is_v2);

            if !is_v2
                || has_corrupted_tail
                || (record_count >= 50 && record_count > model.total_entries() * 3)
            {
                let _ = compact_model(&model, path_ref);
            }

            Ok(model)
        } else {
            UserAdaptiveModel::load_from_file(path_ref)
        }
    } else {
        UserAdaptiveModel::load_from_file(path_ref)
    }
}
