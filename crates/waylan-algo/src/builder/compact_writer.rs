//! 紧凑词典二进制文件段对齐与底层持久化写入器
//!
//! # 设计原理
//! - **实现初衷**：将紧凑双数组 Trie 词库文件的段对齐计算、二进制头部编码与磁盘流式写入
//!   从主构建器中分离，实现关注点解耦。
//! - **核心优势**：职责单一，隔离纯数据结构内存构建与底层文件 I/O；
//!   细化对齐边界与偏移量校验，便于独立进行二进制兼容性测试。
//! - **代价与局限**：调用方需通过装配参数对象传递待写入的切片引用。

use crate::dat::DoubleArrayTrie;
use crate::error::AlgoError;
use crate::table::COMPACT_DICT_MAGIC;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// 紧凑词库二进制持久化装配参数对象
pub struct CompactWritePayload<'a> {
    pub output_path: &'a Path,
    pub dat: &'a DoubleArrayTrie,
    pub key_metas_data: &'a [u8],
    pub candidates_data: &'a [u8],
    pub words: &'a [String],
    pub num_keys: usize,
    pub num_candidates: usize,
}

/// 词表序列化数据包
struct VocabSerializedData {
    vocab_offsets: Vec<u32>,
    sorted_vocab_ids: Vec<u32>,
    vocab_strings: Vec<u8>,
}

/// 各数据段绝对偏移量与填充计算结果
struct SectionOffsets {
    pad1: usize,
    pad2: usize,
    key_metas_offset: u64,
    candidates_offset: u64,
    vocab_offset: u64,
}

/// 组装文件头与各段数据并写入目标磁盘文件
pub fn write_compact_file(payload: CompactWritePayload<'_>) -> Result<(), AlgoError> {
    let vocab_data = serialize_vocab_data(payload.words);
    let offsets = compute_section_offsets(
        payload.dat,
        payload.key_metas_data.len(),
        payload.candidates_data.len(),
    );

    let out_file = File::create(payload.output_path).map_err(|e| AlgoError::DictionaryIo {
        path: PathBuf::from(payload.output_path),
        source: e,
    })?;
    let mut writer = BufWriter::new(out_file);

    write_sections_to_disk(&mut writer, &payload, &vocab_data, &offsets)
}

/// 序列化词表偏移、排序索引与字符串池
fn serialize_vocab_data(words: &[String]) -> VocabSerializedData {
    let vocab_size = words.len();
    let mut vocab_offsets: Vec<u32> = Vec::with_capacity(vocab_size.saturating_add(1));
    let mut vocab_strings: Vec<u8> = Vec::new();
    let mut current_str_offset: u32 = 0;

    for w in words {
        vocab_offsets.push(current_str_offset);
        let b = w.as_bytes();
        vocab_strings.extend_from_slice(b);
        current_str_offset = current_str_offset.saturating_add(b.len() as u32);
    }
    vocab_offsets.push(current_str_offset);

    let mut sorted_vocab_ids: Vec<u32> = (0..vocab_size as u32).collect();
    sorted_vocab_ids.sort_by(|&a, &b| words[a as usize].cmp(&words[b as usize]));

    VocabSerializedData {
        vocab_offsets,
        sorted_vocab_ids,
        vocab_strings,
    }
}

/// 计算各段数据在二进制文件中的绝对偏移与 4 字节边界对齐填充
fn compute_section_offsets(
    dat: &DoubleArrayTrie,
    key_metas_len: usize,
    candidates_len: usize,
) -> SectionOffsets {
    let header_len = 64usize;
    let dat_base_bytes = dat.base.len().saturating_mul(4);
    let dat_check_bytes = dat.check.len().saturating_mul(4);
    let dat_tail_bytes = dat.tail.len();

    let mut current_offset = header_len
        .saturating_add(dat_base_bytes)
        .saturating_add(dat_check_bytes)
        .saturating_add(dat_tail_bytes);
    let pad1 = (4 - (current_offset % 4)) % 4;
    current_offset = current_offset.saturating_add(pad1);

    let key_metas_offset = current_offset as u64;
    current_offset = current_offset.saturating_add(key_metas_len);

    let candidates_offset = current_offset as u64;
    current_offset = current_offset.saturating_add(candidates_len);

    let pad2 = (4 - (current_offset % 4)) % 4;
    current_offset = current_offset.saturating_add(pad2);

    let vocab_offset = current_offset as u64;

    SectionOffsets {
        pad1,
        pad2,
        key_metas_offset,
        candidates_offset,
        vocab_offset,
    }
}

/// 构建二进制词库头部 64 字节数据
fn build_compact_header(payload: &CompactWritePayload<'_>, offsets: &SectionOffsets) -> [u8; 64] {
    let mut header = [0u8; 64];
    header[0..16].copy_from_slice(COMPACT_DICT_MAGIC);
    header[16..20].copy_from_slice(&3u32.to_le_bytes());
    header[20..24].copy_from_slice(&(payload.num_keys as u32).to_le_bytes());
    header[24..28].copy_from_slice(&(payload.num_candidates as u32).to_le_bytes());
    header[28..32].copy_from_slice(&(payload.words.len() as u32).to_le_bytes());
    header[32..36].copy_from_slice(&(payload.dat.base.len() as u32).to_le_bytes());
    header[36..40].copy_from_slice(&(payload.dat.tail.len() as u32).to_le_bytes());
    header[40..48].copy_from_slice(&offsets.key_metas_offset.to_le_bytes());
    header[48..56].copy_from_slice(&offsets.candidates_offset.to_le_bytes());
    header[56..64].copy_from_slice(&offsets.vocab_offset.to_le_bytes());
    header
}

/// 按字节序列化写入各分段并刷新磁盘缓冲区
fn write_sections_to_disk(
    writer: &mut BufWriter<File>,
    payload: &CompactWritePayload<'_>,
    vocab_data: &VocabSerializedData,
    offsets: &SectionOffsets,
) -> Result<(), AlgoError> {
    let io_err = |e| AlgoError::DictionaryIo {
        path: PathBuf::from(payload.output_path),
        source: e,
    };

    let header = build_compact_header(payload, offsets);
    writer.write_all(&header).map_err(io_err)?;

    for &b in &payload.dat.base {
        writer.write_all(&b.to_le_bytes()).map_err(io_err)?;
    }
    for &c in &payload.dat.check {
        writer.write_all(&c.to_le_bytes()).map_err(io_err)?;
    }
    writer.write_all(&payload.dat.tail).map_err(io_err)?;
    if offsets.pad1 > 0 {
        writer.write_all(&vec![0u8; offsets.pad1]).map_err(io_err)?;
    }

    writer.write_all(payload.key_metas_data).map_err(io_err)?;
    writer.write_all(payload.candidates_data).map_err(io_err)?;
    if offsets.pad2 > 0 {
        writer.write_all(&vec![0u8; offsets.pad2]).map_err(io_err)?;
    }

    for &off in &vocab_data.vocab_offsets {
        writer.write_all(&off.to_le_bytes()).map_err(io_err)?;
    }
    for &id in &vocab_data.sorted_vocab_ids {
        writer.write_all(&id.to_le_bytes()).map_err(io_err)?;
    }
    writer
        .write_all(&vocab_data.vocab_strings)
        .map_err(io_err)?;

    writer.flush().map_err(io_err)?;
    Ok(())
}
