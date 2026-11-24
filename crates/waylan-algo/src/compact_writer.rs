//! 紧凑词典二进制文件段对齐与底层持久化写入器
//!
//! # 设计原理
//! - **实现初衷**：将紧凑双数组 Trie 词库文件的段对齐计算、二进制头部编码与磁盘流式写入
//!   从主构建器中分离，实现关注点解耦。
//! - **核心优势**：职责单一，隔离纯数据结构内存构建与底层文件 I/O；
//!   细化对齐边界与偏移量校验，便于独立进行二进制兼容性测试。
//! - **代价与局限**：调用方需通过装配参数对象传递待写入的切片引用。

pub(crate) use crate::compact_key_encoder::{
    KeyCandidateMap, build_combined_keys, encode_keys_and_candidates,
};
use crate::dat::DoubleArrayTrie;
#[cfg(any(feature = "dict-builder", test))]
use crate::error::AlgoError;
use crate::table::COMPACT_DICT_MAGIC;
use std::collections::HashMap;
#[cfg(any(feature = "dict-builder", test))]
use std::fs::File;
#[cfg(any(feature = "dict-builder", test))]
use std::io::BufWriter;
use std::io::Write;
#[cfg(any(feature = "dict-builder", test))]
use std::path::{Path, PathBuf};

/// 紧凑词库二进制持久化装配参数对象
#[cfg(any(feature = "dict-builder", test))]
pub struct CompactWritePayload<'a> {
    pub output_path: &'a Path,
    pub dat: &'a DoubleArrayTrie,
    pub key_metas_data: &'a [u8],
    pub candidates_data: &'a [u8],
    pub words: &'a [String],
    pub num_keys: usize,
    pub num_candidates: usize,
}

/// 紧凑词库二进制编码核心切片视图（同时服务于磁盘写入与纯内存编译）
struct CompactSectionPayload<'a> {
    dat: &'a DoubleArrayTrie,
    key_metas_data: &'a [u8],
    candidates_data: &'a [u8],
    words: &'a [String],
    num_keys: usize,
    num_candidates: usize,
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
#[cfg(any(feature = "dict-builder", test))]
pub fn write_compact_file(payload: CompactWritePayload<'_>) -> Result<(), AlgoError> {
    let section_payload = CompactSectionPayload {
        dat: payload.dat,
        key_metas_data: payload.key_metas_data,
        candidates_data: payload.candidates_data,
        words: payload.words,
        num_keys: payload.num_keys,
        num_candidates: payload.num_candidates,
    };
    let vocab_data = serialize_vocab_data(section_payload.words);
    let offsets = compute_section_offsets(
        section_payload.dat,
        section_payload.key_metas_data.len(),
        section_payload.candidates_data.len(),
    );

    let out_file = File::create(payload.output_path).map_err(|e| AlgoError::DictionaryIo {
        path: PathBuf::from(payload.output_path),
        source: e,
    })?;
    let mut writer = BufWriter::new(out_file);
    let io_err = |e| AlgoError::DictionaryIo {
        path: PathBuf::from(payload.output_path),
        source: e,
    };

    write_sections(&mut writer, &section_payload, &vocab_data, &offsets).map_err(io_err)?;
    writer.flush().map_err(io_err)?;
    Ok(())
}

/// 在纯内存中将已编码的词库段组装为完整紧凑词典字节缓冲区
fn write_compact_bytes(payload: &CompactSectionPayload<'_>) -> Vec<u8> {
    let vocab_data = serialize_vocab_data(payload.words);
    let offsets = compute_section_offsets(
        payload.dat,
        payload.key_metas_data.len(),
        payload.candidates_data.len(),
    );
    let estimated_size = (offsets.vocab_offset as usize)
        .saturating_add(vocab_data.vocab_offsets.len().saturating_mul(4))
        .saturating_add(vocab_data.sorted_vocab_ids.len().saturating_mul(4))
        .saturating_add(vocab_data.vocab_strings.len());
    let mut buf = Vec::with_capacity(estimated_size);
    // 写入内存 Vec<u8> 不会触发底层 I/O 错误
    let _ = write_sections(&mut buf, payload, &vocab_data, &offsets);
    buf
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
fn build_compact_header(payload: &CompactSectionPayload<'_>, offsets: &SectionOffsets) -> [u8; 64] {
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

/// 按字节序列化写入各分段至任意 `Write` 目标
fn write_sections<W: Write>(
    writer: &mut W,
    payload: &CompactSectionPayload<'_>,
    vocab_data: &VocabSerializedData,
    offsets: &SectionOffsets,
) -> std::io::Result<()> {
    let header = build_compact_header(payload, offsets);
    writer.write_all(&header)?;

    for &b in &payload.dat.base {
        writer.write_all(&b.to_le_bytes())?;
    }
    for &c in &payload.dat.check {
        writer.write_all(&c.to_le_bytes())?;
    }
    writer.write_all(&payload.dat.tail)?;
    const ZERO_PAD: [u8; 4] = [0u8; 4];
    if offsets.pad1 > 0 {
        writer.write_all(&ZERO_PAD[..offsets.pad1])?;
    }

    writer.write_all(payload.key_metas_data)?;
    writer.write_all(payload.candidates_data)?;
    if offsets.pad2 > 0 {
        writer.write_all(&ZERO_PAD[..offsets.pad2])?;
    }

    for &off in &vocab_data.vocab_offsets {
        writer.write_all(&off.to_le_bytes())?;
    }
    for &id in &vocab_data.sorted_vocab_ids {
        writer.write_all(&id.to_le_bytes())?;
    }
    writer.write_all(&vocab_data.vocab_strings)?;
    Ok(())
}

/// 在纯内存中将词条映射编译为紧凑词典二进制字节流（用于内存样例库与单测 fixture）
pub(crate) fn compile_entries_to_compact_bytes(
    entries: &HashMap<String, Vec<(String, u32)>>,
) -> Vec<u8> {
    let mut word_to_id: HashMap<&str, u32> = HashMap::new();
    let mut words: Vec<String> = Vec::new();
    let mut dict_map: KeyCandidateMap = HashMap::with_capacity(entries.len());

    for (pinyin, list) in entries {
        let mut cands = Vec::with_capacity(list.len());
        for (text, score) in list {
            let wid = if let Some(&id) = word_to_id.get(text.as_str()) {
                id
            } else {
                let id = words.len() as u32;
                words.push(text.clone());
                word_to_id.insert(text.as_str(), id);
                id
            };
            cands.push((wid, *score));
        }
        cands.sort_by_key(|b| std::cmp::Reverse(b.1));
        dict_map.insert(pinyin.clone(), cands);
    }

    // 内存 fixture 保留全部简拼候选（min_acronym_score = 1），不设 200 分阈值截断
    let (combined_keys, sorted_keys) = build_combined_keys(dict_map, &words, 0, 1);
    let key_values: Vec<u32> = (0..sorted_keys.len() as u32).collect();
    let dat = DoubleArrayTrie::build(&sorted_keys, &key_values);
    let (key_metas_data, candidates_data, num_candidates) =
        encode_keys_and_candidates(&combined_keys, &sorted_keys);

    write_compact_bytes(&CompactSectionPayload {
        dat: &dat,
        key_metas_data: &key_metas_data,
        candidates_data: &candidates_data,
        words: &words,
        num_keys: sorted_keys.len(),
        num_candidates,
    })
}
