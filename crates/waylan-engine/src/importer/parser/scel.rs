//! 搜狗细胞词库 (.scel) 二进制解析器

use std::collections::HashMap;

use log::warn;
use waylan_algo::types::ImportedDictEntry;
use waylan_core::MAX_IMPORT_FILE_BYTES;

use crate::error::DictImportError;

/// 搜狗细胞词库标准文件头前 4 字节魔数
const SCEL_HEADER_MAGIC: [u8; 4] = [0x40, 0x15, 0x00, 0x00];
/// 搜狗细胞词库拼音表区 (0x1540) 4 字节魔数
const SCEL_PY_MAGIC: [u8; 4] = [0x9D, 0x01, 0x00, 0x00];

/// 搜狗细胞词库同音词组批量解析参数
struct ScelWordBatchParams<'a> {
    data: &'a [u8],
    cursor: &'a mut usize,
    same_pinyin_count: usize,
    full_pinyin: &'a str,
    entries: &'a mut Vec<ImportedDictEntry>,
}

/// 解析搜狗细胞词库二进制格式 (.scel)
pub(super) fn parse_sogou_scel(data: &[u8]) -> Result<Vec<ImportedDictEntry>, DictImportError> {
    let pinyin_table_offset = 0x1540;
    let words_offset = 0x2628;

    validate_scel_header(data, pinyin_table_offset, words_offset)?;

    let pinyin_table = read_scel_pinyin_table(&data[pinyin_table_offset..words_offset])?;

    let mut entries = Vec::new();
    let mut cursor = words_offset;
    let mut truncated = false;

    while let Some((same_pinyin_count, full_pinyin)) =
        parse_scel_pinyin_group(data, &mut cursor, &pinyin_table)
    {
        if !parse_scel_words_for_pinyin(ScelWordBatchParams {
            data,
            cursor: &mut cursor,
            same_pinyin_count,
            full_pinyin: &full_pinyin,
            entries: &mut entries,
        }) {
            truncated = true;
            break;
        }
    }

    if truncated {
        warn!(
            "[微澜词库] 搜狗 .scel 词库在偏移 0x{:X} 处遇到截断或越界，已安全保留此前完整解析的 {} 条词目。",
            cursor,
            entries.len()
        );
        if entries.is_empty() {
            return Err(DictImportError::ParseFailed {
                reason: "搜狗细胞词库词条区损坏或截断，未解析出有效词目".to_string(),
            });
        }
    }

    Ok(entries)
}

/// 校验搜狗细胞词库基本长度与双重魔数头
fn validate_scel_header(
    data: &[u8],
    pinyin_offset: usize,
    words_offset: usize,
) -> Result<(), DictImportError> {
    if data.len() < words_offset || data.len() > MAX_IMPORT_FILE_BYTES as usize {
        return Err(DictImportError::ParseFailed {
            reason: "无效或损坏的搜狗细胞词库文件 (体积不在合法范围内)".to_string(),
        });
    }

    if data[0..4] != SCEL_HEADER_MAGIC || data[pinyin_offset..pinyin_offset + 4] != SCEL_PY_MAGIC {
        return Err(DictImportError::ParseFailed {
            reason: "搜狗细胞词库魔数头校验失败".to_string(),
        });
    }
    Ok(())
}

/// 从数据切片中提取搜狗拼音索引表
fn read_scel_pinyin_table(slice: &[u8]) -> Result<HashMap<usize, String>, DictImportError> {
    let mut map = HashMap::new();
    let mut cursor: usize = if slice.len() >= 4 && slice[0..4] == SCEL_PY_MAGIC {
        4
    } else {
        0
    };

    while cursor.saturating_add(4) <= slice.len() {
        let index = u16::from_le_bytes([slice[cursor], slice[cursor + 1]]) as usize;
        cursor = cursor.saturating_add(2);
        let len = u16::from_le_bytes([slice[cursor], slice[cursor + 1]]) as usize;
        cursor = cursor.saturating_add(2);

        let end = cursor.saturating_add(len);
        if len == 0 || end > slice.len() {
            break;
        }

        if let Some(py) = decode_utf16le_checked(&slice[cursor..end]) {
            map.insert(index, py);
        }
        cursor = end;
    }

    Ok(map)
}

/// 解析单个词条组的拼音序列，返回 (同音词数, 组合拼音)
fn parse_scel_pinyin_group(
    data: &[u8],
    cursor: &mut usize,
    pinyin_table: &HashMap<usize, String>,
) -> Option<(usize, String)> {
    if cursor.saturating_add(4) > data.len() {
        return None;
    }
    let same_pinyin_count = u16::from_le_bytes([data[*cursor], data[*cursor + 1]]) as usize;
    *cursor = cursor.saturating_add(2);

    let py_indices_byte_len = u16::from_le_bytes([data[*cursor], data[*cursor + 1]]) as usize;
    *cursor = cursor.saturating_add(2);

    if same_pinyin_count == 0
        || py_indices_byte_len == 0
        || !py_indices_byte_len.is_multiple_of(2)
        || cursor.saturating_add(py_indices_byte_len) > data.len()
    {
        return None;
    }

    let mut py_list = Vec::with_capacity(py_indices_byte_len / 2);
    for i in (0..py_indices_byte_len).step_by(2) {
        let idx_pos = cursor.saturating_add(i);
        let idx = u16::from_le_bytes([data[idx_pos], data[idx_pos + 1]]) as usize;
        if let Some(py_str) = pinyin_table.get(&idx)
            && !py_str.is_empty()
        {
            py_list.push(py_str.as_str());
        }
    }
    *cursor = cursor.saturating_add(py_indices_byte_len);

    Some((same_pinyin_count, py_list.join("'")))
}

/// 解析指定同音词集合并追加到词条结果列表，若数据截断返回 false
fn parse_scel_words_for_pinyin(params: ScelWordBatchParams<'_>) -> bool {
    let ScelWordBatchParams {
        data,
        cursor,
        same_pinyin_count,
        full_pinyin,
        entries,
    } = params;
    for _ in 0..same_pinyin_count {
        if !parse_scel_single_word(data, cursor, full_pinyin, entries) {
            return false;
        }
    }
    true
}

/// 解析单个搜狗细胞词库词条（UTF-16 编码词文本 + 定长扩展信息块）
fn parse_scel_single_word(
    data: &[u8],
    cursor: &mut usize,
    full_pinyin: &str,
    entries: &mut Vec<ImportedDictEntry>,
) -> bool {
    if cursor.saturating_add(2) > data.len() {
        return false;
    }
    let word_byte_len = u16::from_le_bytes([data[*cursor], data[*cursor + 1]]) as usize;
    *cursor = cursor.saturating_add(2);

    let word_end = cursor.saturating_add(word_byte_len);
    if word_end > data.len() {
        return false;
    }

    let Some(word_text) = decode_utf16le_checked(&data[*cursor..word_end]) else {
        return false;
    };
    *cursor = word_end;

    if cursor.saturating_add(2) > data.len() {
        return false;
    }
    let ext_len = u16::from_le_bytes([data[*cursor], data[*cursor + 1]]) as usize;
    let next_cursor = cursor.saturating_add(2).saturating_add(ext_len);
    if next_cursor > data.len() {
        return false;
    }
    *cursor = next_cursor;

    if !word_text.is_empty() && !full_pinyin.is_empty() {
        entries.push(ImportedDictEntry {
            text: word_text,
            pinyin: full_pinyin.to_string(),
            weight: 200,
        });
    }
    true
}

/// 严格校验偶数字节长度并将 UTF-16LE 字节流解码为 `String`（拒绝奇数尾部截断）
fn decode_utf16le_checked(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = String::with_capacity(bytes.len() / 2);
    let iter = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(u16::from_le_bytes);
    for ch in char::decode_utf16(iter) {
        out.push(ch.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    Some(out)
}
