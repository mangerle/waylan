//! 用户词库预写日志（WAL）参数对象与二进制帧序列化工具

use std::path::Path;

use log::warn;

use super::constants::WAL_CHECKSUM_SEED;

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

/// 用户词对搭配追加参数对象（收敛 5 参数签名并消除 `prev`/`curr` 顺序颠倒风险）
#[derive(Debug, Clone, Copy)]
pub struct UserDictPairParams<'a> {
    /// WAL 日志文件路径
    pub path: &'a Path,
    /// 前导词条
    pub prev: &'a str,
    /// 当前词条
    pub curr: &'a str,
    /// 记录时间戳（秒级 UNIX 时间）
    pub timestamp: u64,
}

/// 用户精准忘词追加参数对象
#[derive(Debug, Clone, Copy)]
pub struct UserDictForgetParams<'a> {
    /// WAL 日志文件路径
    pub path: &'a Path,
    /// 需要忘记的词条文本
    pub text: &'a str,
    /// 可选关联拼音键
    pub pinyin: Option<&'a str>,
    /// 记录时间戳（秒级 UNIX 时间）
    pub timestamp: u64,
}

/// 原地编码单条用户上屏 WAL 帧二进制数据包写入复用的缓冲区（供追加与压缩模块共用）
pub(super) fn serialize_wal_frame_into(
    buf: &mut Vec<u8>,
    text: &str,
    pinyin: &str,
    timestamp: u64,
    count: u32,
) -> bool {
    let py_bytes = pinyin.as_bytes();
    let text_bytes = text.as_bytes();
    if py_bytes.len() > u16::MAX as usize || text_bytes.len() > u16::MAX as usize {
        warn!(
            "待写入的用户词条长度超过 65535 字节上限，跳过写入: py_len={}, text_len={}",
            py_bytes.len(),
            text_bytes.len()
        );
        return false;
    }

    let py_len = py_bytes.len() as u16;
    let text_len = text_bytes.len() as u16;

    buf.clear();
    buf.reserve(20 + py_bytes.len() + text_bytes.len());
    buf.extend_from_slice(&timestamp.to_le_bytes());
    buf.extend_from_slice(&count.to_le_bytes());
    buf.extend_from_slice(&py_len.to_le_bytes());
    buf.extend_from_slice(&text_len.to_le_bytes());
    buf.extend_from_slice(py_bytes);
    buf.extend_from_slice(text_bytes);

    let checksum: u32 = buf
        .iter()
        .fold(WAL_CHECKSUM_SEED, |acc, &b| acc.wrapping_add(b as u32));
    buf.extend_from_slice(&checksum.to_le_bytes());
    true
}

/// 序列化单条用户上屏 WAL 帧二进制数据包（包含校验和）
pub(super) fn serialize_wal_frame(
    text: &str,
    pinyin: &str,
    timestamp: u64,
    count: u32,
) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(20 + pinyin.len() + text.len());
    serialize_wal_frame_into(&mut buf, text, pinyin, timestamp, count).then_some(buf)
}
