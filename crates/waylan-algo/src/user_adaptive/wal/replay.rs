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
use crate::user_adaptive::wal::constants::{
    WAL_CHECKSUM_SEED, WAL_FORGET_PREFIX, WAL_MAGIC, WAL_MAGIC_V1, WAL_PAIR_PREFIX,
};
use crate::user_adaptive::wal::lock::ProcessFileLock;
use crate::user_adaptive::{CommitCountParams, UserAdaptiveModel, UserPairEntry};

/// 从二进制缓冲区重放 WAL 记录帧（公共重放逻辑）
pub(crate) fn replay_wal_records(buf: &[u8], is_v2: bool) -> (UserAdaptiveModel, bool, usize) {
    let mut model = UserAdaptiveModel::new();
    let mut record_count = 0;
    let mut cursor = 0;
    let mut has_corrupted_tail = false;
    let header_len = wal_header_len(is_v2);

    while cursor + header_len <= buf.len() {
        match decode_wal_record(buf, &mut cursor, is_v2) {
            DecodedWalRecord::Valid {
                timestamp,
                count,
                py,
                text,
            } => {
                if count == 0 {
                    if let Some(target_py) = py.strip_prefix(WAL_FORGET_PREFIX) {
                        let py_opt = (!target_py.is_empty()).then_some(target_py);
                        model.forget_word(text, py_opt);
                        record_count += 1;
                    }
                } else if let Some(prev) = py.strip_prefix(WAL_PAIR_PREFIX) {
                    if !prev.is_empty() && !text.is_empty() {
                        if count == 1 {
                            model.record_pair(prev, text, timestamp);
                        } else {
                            model.max_timestamp = model.max_timestamp.max(timestamp);
                            model.pairs.entry(prev.to_string()).or_default().insert(
                                text.to_string(),
                                UserPairEntry {
                                    count,
                                    last_used_timestamp: timestamp,
                                },
                            );
                        }
                        record_count += 1;
                    }
                } else if !py.is_empty() && !text.is_empty() {
                    model.record_commit_count(&CommitCountParams {
                        text,
                        pinyin: py,
                        current_timestamp: timestamp,
                        count,
                    });
                    record_count += 1;
                }
            }
            DecodedWalRecord::Broken => {
                has_corrupted_tail = true;
                break;
            }
        }
    }

    (model, has_corrupted_tail, record_count)
}

/// 单条 WAL 记录帧的解码结果
enum DecodedWalRecord<'a> {
    /// 正常解码出的词条内容（`py` 为拼音键，`text` 为词条文本）
    Valid {
        /// 记录时间戳（秒级 UNIX 时间）
        timestamp: u64,
        /// 记录携带的累计上屏次数
        count: u32,
        /// 拼音键
        py: &'a str,
        /// 词条文本
        text: &'a str,
    },
    /// 帧内容损坏或被截断，需终止后续失步帧解析
    Broken,
}

/// 单条 WAL 帧的头部字段
struct WalRecordHeader {
    /// 记录时间戳（秒级 UNIX 时间）
    timestamp: u64,
    /// 记录携带的累计上屏次数
    count: u32,
    /// 拼音键字节长度
    py_len: usize,
    /// 词条文本字节长度
    text_len: usize,
}

/// 获取指定 WAL 帧格式的头部长度（V1 为 12 字节，V2 为 16 字节）
fn wal_header_len(is_v2: bool) -> usize {
    if is_v2 { 16 } else { 12 }
}

/// 解析单条 WAL 帧的头部字段（兼容 V1 与 V2 两种帧头布局）
fn parse_wal_record_header(buf: &[u8], cursor: usize, is_v2: bool) -> WalRecordHeader {
    let timestamp = u64::from_le_bytes(buf[cursor..cursor + 8].try_into().unwrap_or([0; 8]));
    let (count, py_len, text_len) = if is_v2 {
        let count = u32::from_le_bytes(buf[cursor + 8..cursor + 12].try_into().unwrap_or([0; 4]));
        let py_len =
            u16::from_le_bytes(buf[cursor + 12..cursor + 14].try_into().unwrap_or([0; 2])) as usize;
        let text_len =
            u16::from_le_bytes(buf[cursor + 14..cursor + 16].try_into().unwrap_or([0; 2])) as usize;
        (count, py_len, text_len)
    } else {
        let py_len =
            u16::from_le_bytes(buf[cursor + 8..cursor + 10].try_into().unwrap_or([0; 2])) as usize;
        let text_len =
            u16::from_le_bytes(buf[cursor + 10..cursor + 12].try_into().unwrap_or([0; 2])) as usize;
        (1, py_len, text_len)
    };
    WalRecordHeader {
        timestamp,
        count,
        py_len,
        text_len,
    }
}

/// 计算 WAL 帧负载的累加校验和（带非零初始种子，杜绝全零块绕过）
fn compute_wal_checksum(payload: &[u8]) -> u32 {
    payload
        .iter()
        .fold(WAL_CHECKSUM_SEED, |acc, &b| acc.wrapping_add(b as u32))
}

/// 解码单条 WAL 记录帧并校验累加校验和
///
/// 解析游标在成功解码时前移至下一帧起点；帧被截断或校验和不匹配时返回 [`DecodedWalRecord::Broken`]。
fn decode_wal_record<'a>(buf: &'a [u8], cursor: &mut usize, is_v2: bool) -> DecodedWalRecord<'a> {
    let record_start = *cursor;
    let header = parse_wal_record_header(buf, record_start, is_v2);
    *cursor += wal_header_len(is_v2);

    if *cursor + header.py_len + header.text_len + 4 > buf.len() {
        warn!("[用户词库 WAL] 检测到文件末尾记录截断损坏，安全截断并在后续自动压缩");
        return DecodedWalRecord::Broken;
    }

    let py_bytes = &buf[*cursor..*cursor + header.py_len];
    *cursor += header.py_len;
    let text_bytes = &buf[*cursor..*cursor + header.text_len];
    *cursor += header.text_len;

    let expected_checksum =
        u32::from_le_bytes(buf[*cursor..*cursor + 4].try_into().unwrap_or([0; 4]));
    *cursor += 4;

    let payload = &buf[record_start..*cursor - 4];
    let actual_checksum = compute_wal_checksum(payload);
    if actual_checksum != expected_checksum {
        warn!(
            "[用户词库 WAL] 记录校验和不匹配 (预期: {:#x}, 实际: {:#x})，终止后续失步帧解析并执行归并清理",
            expected_checksum, actual_checksum
        );
        return DecodedWalRecord::Broken;
    }

    DecodedWalRecord::Valid {
        timestamp: header.timestamp,
        count: header.count,
        py: std::str::from_utf8(py_bytes).unwrap_or(""),
        text: std::str::from_utf8(text_bytes).unwrap_or(""),
    }
}

/// 在跨进程共享读锁保护下读取并解析 WAL 文件
fn read_and_replay_file(path_ref: &Path) -> Result<(UserAdaptiveModel, bool), AlgoError> {
    let _shared_lock =
        ProcessFileLock::acquire_shared(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

    read_and_replay_file_unlocked(path_ref)
}

/// 在调用方已持有排他锁的前提下读取并解析 WAL 文件
fn read_and_replay_file_unlocked(path_ref: &Path) -> Result<(UserAdaptiveModel, bool), AlgoError> {
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

            let need_compact = !is_v2
                || has_corrupted_tail
                || (record_count >= 50 && record_count > model.total_entries() * 3);

            Ok((model, need_compact))
        } else {
            Ok((UserAdaptiveModel::load_from_file(path_ref)?, false))
        }
    } else {
        Ok((UserAdaptiveModel::load_from_file(path_ref)?, false))
    }
}

/// 从 WAL 日志文件中重放还原用户词库模型，向下兼容旧版全量文件与旧版 V1 WAL
pub fn load_or_replay<P: AsRef<Path>>(path: P) -> Result<UserAdaptiveModel, AlgoError> {
    let path_ref = path.as_ref();
    if !path_ref.exists() {
        return Ok(UserAdaptiveModel::new());
    }

    let (model, need_compact) = read_and_replay_file(path_ref)?;

    // 共享读锁在 read_and_replay_file 退出时已析构释放，此处若需要归并压缩则能安全获取排他写锁，杜绝自死锁
    if need_compact && let Err(e) = compact_model(&model, path_ref) {
        warn!(
            "[用户词库 WAL] 重放后自动归并失败，保留现有日志继续运行，路径: {:?}，原因: {}",
            path_ref, e
        );
    }

    Ok(model)
}

/// 在调用方已持有排他锁时重放 WAL，避免同一进程重复加锁造成自死锁
pub(crate) fn load_or_replay_without_lock(path: &Path) -> Result<UserAdaptiveModel, AlgoError> {
    if !path.exists() {
        return Ok(UserAdaptiveModel::new());
    }
    read_and_replay_file_unlocked(path).map(|(model, _)| model)
}
