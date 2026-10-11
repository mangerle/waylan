use crate::builder::normalize_pinyin_tones;
use crate::error::AlgoError;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// 搜狗细胞词库最大允许文件体积上限 (100 MB)
const MAX_SCEL_SIZE: usize = 100 * 1024 * 1024;
/// 搜狗细胞词库标准文件头前 4 字节魔数
const SCEL_HEADER_MAGIC: [u8; 4] = [0x40, 0x15, 0x00, 0x00];
/// 搜狗细胞词库拼音表区 (0x1540) 4 字节魔数
const SCEL_PY_MAGIC: [u8; 4] = [0x9D, 0x01, 0x00, 0x00];

/// 词库条目结构 (文本, 拼音序列, 基础频次)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedDictEntry {
    pub text: String,
    pub pinyin: String,
    pub weight: u32,
}

/// 第三方词库导入与转换器 (支持 Rime YAML / 搜狗细胞词库 .scel / 纯文本 .txt)
pub struct DictImporter;

impl DictImporter {
    /// 自动根据文件后缀解析导入词库文件
    pub fn import_file(path: &Path) -> Result<Vec<ImportedDictEntry>, AlgoError> {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_lowercase();

        match extension.as_str() {
            "scel" => {
                let meta = fs::metadata(path).map_err(|e| AlgoError::DictionaryIo {
                    path: path.to_path_buf(),
                    source: e,
                })?;
                if meta.len() > MAX_SCEL_SIZE as u64 {
                    return Err(AlgoError::DictParseFailed {
                        reason: format!("搜狗细胞词库文件体积超出 100MB 安全上限: {}", meta.len()),
                    });
                }
                let data = fs::read(path).map_err(|e| AlgoError::DictionaryIo {
                    path: path.to_path_buf(),
                    source: e,
                })?;
                Self::parse_sogou_scel(&data)
            }
            "yaml" | "yml" => {
                let content = fs::read_to_string(path).map_err(|e| AlgoError::DictionaryIo {
                    path: path.to_path_buf(),
                    source: e,
                })?;
                Self::parse_rime_yaml(&content)
            }
            "txt" => {
                let content = fs::read_to_string(path).map_err(|e| AlgoError::DictionaryIo {
                    path: path.to_path_buf(),
                    source: e,
                })?;
                Self::parse_plain_text(&content)
            }
            _ => Err(AlgoError::UnsupportedDictFormat {
                path: path.to_path_buf(),
                format: extension,
            }),
        }
    }

    /// 解析 Rime 词典 YAML 格式（兼容标准 `...` 表头分隔与无 `...` 标记的纯 TSV 词表）
    pub fn parse_rime_yaml(content: &str) -> Result<Vec<ImportedDictEntry>, AlgoError> {
        let mut entries = Vec::new();
        let has_header_sep = content.lines().any(|l| l.trim() == "...");
        let mut past_header = !has_header_sep;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line == "---" {
                continue;
            }

            // Rime 词库表头以 "..." 分割正文
            if line == "..." {
                past_header = true;
                continue;
            }

            if !past_header {
                continue;
            }

            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                let text = parts[0].trim().to_string();
                let pinyin = normalize_pinyin_tones(parts[1].trim());
                let weight = if parts.len() >= 3 {
                    parts[2].trim().parse::<u32>().unwrap_or(100)
                } else {
                    100
                };

                if !text.is_empty() && !pinyin.is_empty() {
                    entries.push(ImportedDictEntry {
                        text,
                        pinyin,
                        weight,
                    });
                }
            }
        }

        Ok(entries)
    }

    /// 解析纯文本格式（词条 拼音 [权重] 或 词条\t拼音）
    pub fn parse_plain_text(content: &str) -> Result<Vec<ImportedDictEntry>, AlgoError> {
        let mut entries = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = if line.contains('\t') {
                line.split('\t').collect()
            } else {
                line.split_whitespace().collect()
            };

            if parts.len() >= 2 {
                let text = parts[0].trim().to_string();
                let pinyin = normalize_pinyin_tones(parts[1].trim());
                let weight = if parts.len() >= 3 {
                    parts[2].trim().parse::<u32>().unwrap_or(100)
                } else {
                    100
                };

                if !text.is_empty() && !pinyin.is_empty() {
                    entries.push(ImportedDictEntry {
                        text,
                        pinyin,
                        weight,
                    });
                }
            }
        }
        Ok(entries)
    }

    /// 解析搜狗细胞词库二进制格式 (.scel)
    pub fn parse_sogou_scel(data: &[u8]) -> Result<Vec<ImportedDictEntry>, AlgoError> {
        let pinyin_table_offset = 0x1540;
        let words_offset = 0x2628;

        // 校验基本文件长度与最大体积上限
        if data.len() < words_offset || data.len() > MAX_SCEL_SIZE {
            return Err(AlgoError::DictParseFailed {
                reason: "无效或损坏的搜狗细胞词库文件 (体积不在合法范围内)".to_string(),
            });
        }

        // 校验 SCEL 文件头魔数或拼音表区魔数标识
        if data[0..4] != SCEL_HEADER_MAGIC
            && data[pinyin_table_offset..pinyin_table_offset + 4] != SCEL_PY_MAGIC
        {
            return Err(AlgoError::DictParseFailed {
                reason: "搜狗细胞词库魔数头校验失败".to_string(),
            });
        }

        // 1. 解析拼音映射表（限定在 [0x1540..0x2628] 区域内，防止越界读取词条区）
        let pinyin_table = Self::read_scel_pinyin_table(&data[pinyin_table_offset..words_offset])?;

        // 2. 解析词条正文（起始偏移 0x2628）
        let mut entries = Vec::new();
        let mut cursor = words_offset;

        while cursor.saturating_add(4) <= data.len() {
            // 同音词数量
            let same_pinyin_count = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
            cursor = cursor.saturating_add(2);

            // 拼音索引序列长度（字节数，每个索引占 2 字节）
            let py_indices_byte_len = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
            cursor = cursor.saturating_add(2);

            if same_pinyin_count == 0
                || py_indices_byte_len == 0
                || cursor.saturating_add(py_indices_byte_len) > data.len()
            {
                break;
            }

            let mut py_list = Vec::with_capacity(py_indices_byte_len / 2);
            for i in (0..py_indices_byte_len).step_by(2) {
                let idx_pos = cursor.saturating_add(i);
                if idx_pos.saturating_add(2) <= data.len() {
                    let idx = u16::from_le_bytes([data[idx_pos], data[idx_pos + 1]]) as usize;
                    if let Some(py_str) = pinyin_table.get(&idx) {
                        if !py_str.is_empty() {
                            py_list.push(py_str.as_str());
                        }
                    }
                }
            }
            cursor = cursor.saturating_add(py_indices_byte_len);

            let full_pinyin = py_list.join("'");

            // 读取紧跟其后的同音词列表
            let mut corrupted = false;
            for _ in 0..same_pinyin_count {
                if cursor.saturating_add(2) > data.len() {
                    corrupted = true;
                    break;
                }
                let word_byte_len = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
                cursor = cursor.saturating_add(2);

                let word_end = cursor.saturating_add(word_byte_len);
                if word_end > data.len() {
                    corrupted = true;
                    break;
                }

                let word_bytes = &data[cursor..word_end];
                let u16_chars: Vec<u16> = word_bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let word_text = String::from_utf16_lossy(&u16_chars);
                cursor = word_end;

                // 跳过扩展信息（通常包含权重、词频说明，占 12 字节左右），严格防御畸形 ext_len 越界
                if cursor.saturating_add(2) > data.len() {
                    corrupted = true;
                    break;
                }
                let ext_len = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
                let next_cursor = cursor.saturating_add(2).saturating_add(ext_len);
                if next_cursor > data.len() {
                    corrupted = true;
                    break;
                }
                cursor = next_cursor;

                if !word_text.is_empty() && !full_pinyin.is_empty() {
                    entries.push(ImportedDictEntry {
                        text: word_text,
                        pinyin: full_pinyin.clone(),
                        weight: 200,
                    });
                }
            }

            if corrupted {
                break;
            }
        }

        Ok(entries)
    }

    /// 从数据切片中提取搜狗拼音索引表
    fn read_scel_pinyin_table(slice: &[u8]) -> Result<HashMap<usize, String>, AlgoError> {
        let mut map = HashMap::new();
        // 若包含标准 4 字节头部魔数 [0x9D, 0x01, 0x00, 0x00]，跳过头部
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

            let u16_chars: Vec<u16> = slice[cursor..end]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let py = String::from_utf16_lossy(&u16_chars);
            cursor = end;

            map.insert(index, py);
        }

        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rime_yaml() {
        let sample = "---\nname: sample\nversion: \"1.0\"\n...\n汉字\than'zi\t500\n微澜\twei'lan\n";
        let res = DictImporter::parse_rime_yaml(sample).unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].text, "汉字");
        assert_eq!(res[0].pinyin, "han'zi");
        assert_eq!(res[0].weight, 500);
        assert_eq!(res[1].text, "微澜");
        assert_eq!(res[1].pinyin, "wei'lan");

        // 兼容无 ... 头部分隔符且带大小写声调的 Rime TSV 词表
        let headerless = "长安\tCháng''ān\t800\n";
        let res2 = DictImporter::parse_rime_yaml(headerless).unwrap();
        assert_eq!(res2.len(), 1);
        assert_eq!(res2[0].text, "长安");
        assert_eq!(res2[0].pinyin, "chang'an");
    }

    #[test]
    fn test_parse_plain_text() {
        let sample = "测试 ce'shi 100\n输入法 shū''rù'fǎ 200\n";
        let res = DictImporter::parse_plain_text(sample).unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].text, "测试");
        assert_eq!(res[1].text, "输入法");
        assert_eq!(res[1].pinyin, "shu'ru'fa");
    }

    #[test]
    fn test_parse_sogou_scel_valid_and_corrupted_bounds() {
        // 1. 过小数据应返回明确的 DictParseFailed 错误
        let tiny_data = vec![0u8; 100];
        assert!(DictImporter::parse_sogou_scel(&tiny_data).is_err());

        // 2. 构造合法的合成 SCEL 二进制结构
        let mut scel = vec![0u8; 0x2628];
        // 在 0x1540 写入拼音表：索引 0 -> "wei", 索引 1 -> "lan"
        let mut py_cursor = 0x1540;
        scel[py_cursor..py_cursor + 4].copy_from_slice(&[0x9D, 0x01, 0x00, 0x00]);
        py_cursor += 4;
        // 索引 0: "wei"
        scel[py_cursor..py_cursor + 2].copy_from_slice(&0u16.to_le_bytes());
        let wei_u16: Vec<u8> = "wei".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        scel[py_cursor + 2..py_cursor + 4].copy_from_slice(&(wei_u16.len() as u16).to_le_bytes());
        scel[py_cursor + 4..py_cursor + 4 + wei_u16.len()].copy_from_slice(&wei_u16);
        py_cursor += 4 + wei_u16.len();

        // 索引 1: "lan"
        scel[py_cursor..py_cursor + 2].copy_from_slice(&1u16.to_le_bytes());
        let lan_u16: Vec<u8> = "lan".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        scel[py_cursor + 2..py_cursor + 4].copy_from_slice(&(lan_u16.len() as u16).to_le_bytes());
        scel[py_cursor + 4..py_cursor + 4 + lan_u16.len()].copy_from_slice(&lan_u16);

        // 在 0x2628 处追加第一个合法词条组：1 个同音词，拼音索引 [0, 1] (4 字节)，词 "微澜"
        scel.extend_from_slice(&1u16.to_le_bytes()); // same_pinyin_count = 1
        scel.extend_from_slice(&4u16.to_le_bytes()); // py_indices_byte_len = 4
        scel.extend_from_slice(&0u16.to_le_bytes()); // idx 0 ("wei")
        scel.extend_from_slice(&1u16.to_le_bytes()); // idx 1 ("lan")

        let word_u16: Vec<u8> = "微澜"
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        scel.extend_from_slice(&(word_u16.len() as u16).to_le_bytes());
        scel.extend_from_slice(&word_u16);
        // ext_len = 4 字节扩展数据
        scel.extend_from_slice(&4u16.to_le_bytes());
        scel.extend_from_slice(&[1, 2, 3, 4]);

        // 追加第二个畸变词条组（ext_len 故意写成 0xFFFF 越界，验证防御机制不会 panic 且丢弃残损项）
        scel.extend_from_slice(&1u16.to_le_bytes());
        scel.extend_from_slice(&2u16.to_le_bytes());
        scel.extend_from_slice(&0u16.to_le_bytes());
        scel.extend_from_slice(&(word_u16.len() as u16).to_le_bytes());
        scel.extend_from_slice(&word_u16);
        scel.extend_from_slice(&0xFFFFu16.to_le_bytes()); // 恶意超长 ext_len

        let parsed = DictImporter::parse_sogou_scel(&scel).expect("解析含尾部畸变的SCEL不应崩溃");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].text, "微澜");
        assert_eq!(parsed[0].pinyin, "wei'lan");
    }
}
