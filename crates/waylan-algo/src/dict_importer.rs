use std::fs;
use std::path::Path;

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
    pub fn import_file(path: &Path) -> Result<Vec<ImportedDictEntry>, Box<dyn std::error::Error>> {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_lowercase();

        match extension.as_str() {
            "scel" => {
                let data = fs::read(path)?;
                Self::parse_sogou_scel(&data)
            }
            "yaml" | "yml" => {
                let content = fs::read_to_string(path)?;
                Self::parse_rime_yaml(&content)
            }
            "txt" => {
                let content = fs::read_to_string(path)?;
                Self::parse_plain_text(&content)
            }
            _ => Err("不支持的词库文件格式，仅支持 .scel, .yaml, .txt".into()),
        }
    }

    /// 解析 Rime 词典 YAML 格式
    pub fn parse_rime_yaml(content: &str) -> Result<Vec<ImportedDictEntry>, Box<dyn std::error::Error>> {
        let mut entries = Vec::new();
        let mut past_header = false;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
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
                let pinyin = parts[1].trim().replace(' ', "'");
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
    pub fn parse_plain_text(content: &str) -> Result<Vec<ImportedDictEntry>, Box<dyn std::error::Error>> {
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
                let pinyin = parts[1].trim().replace(' ', "'");
                let weight = if parts.len() >= 3 {
                    parts[2].trim().parse::<u32>().unwrap_or(100)
                } else {
                    100
                };

                entries.push(ImportedDictEntry {
                    text,
                    pinyin,
                    weight,
                });
            }
        }
        Ok(entries)
    }

    /// 解析搜狗细胞词库二进制格式 (.scel)
    pub fn parse_sogou_scel(data: &[u8]) -> Result<Vec<ImportedDictEntry>, Box<dyn std::error::Error>> {
        // 校验基本文件长度
        if data.len() < 0x2628 {
            return Err("无效或损坏的搜狗细胞词库文件 (体积过小)".into());
        }

        // 1. 解析拼音映射表（起始偏移 0x1540）
        let pinyin_table_offset = 0x1540;
        let pinyin_table = Self::read_scel_pinyin_table(&data[pinyin_table_offset..])?;

        // 2. 解析词条正文（起始偏移 0x2628）
        let words_offset = 0x2628;
        let mut entries = Vec::new();
        let mut cursor = words_offset;

        while cursor + 4 <= data.len() {
            // 同音词数量
            let same_pinyin_count = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
            cursor += 2;

            // 拼音索引序列长度（字节数，每个索引占 2 字节）
            let py_indices_byte_len = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
            cursor += 2;

            if cursor + py_indices_byte_len > data.len() {
                break;
            }

            let mut py_list = Vec::new();
            for i in (0..py_indices_byte_len).step_by(2) {
                if cursor + i + 1 < data.len() {
                    let idx = u16::from_le_bytes([data[cursor + i], data[cursor + i + 1]]) as usize;
                    if let Some(py_str) = pinyin_table.get(&idx) {
                        py_list.push(py_str.as_str());
                    }
                }
            }
            cursor += py_indices_byte_len;

            let full_pinyin = py_list.join("'");

            // 读取紧跟其后的同音词列表
            for _ in 0..same_pinyin_count {
                if cursor + 2 > data.len() {
                    break;
                }
                let word_byte_len = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
                cursor += 2;

                if cursor + word_byte_len > data.len() {
                    break;
                }

                let word_bytes = &data[cursor..cursor + word_byte_len];
                let u16_chars: Vec<u16> = word_bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let word_text = String::from_utf16_lossy(&u16_chars);
                cursor += word_byte_len;

                // 跳过扩展信息（通常包含权重、词频说明，占 12 字节左右）
                if cursor + 2 <= data.len() {
                    let ext_len = u16::from_le_bytes([data[cursor], data[cursor + 1]]) as usize;
                    cursor += 2 + ext_len;
                }

                if !word_text.is_empty() && !full_pinyin.is_empty() {
                    entries.push(ImportedDictEntry {
                        text: word_text,
                        pinyin: full_pinyin.clone(),
                        weight: 200,
                    });
                }
            }
        }

        Ok(entries)
    }

    /// 从数据切片中提取搜狗拼音索引表
    fn read_scel_pinyin_table(
        slice: &[u8],
    ) -> Result<std::collections::HashMap<usize, String>, Box<dyn std::error::Error>> {
        let mut map = std::collections::HashMap::new();
        let mut cursor = 0;

        while cursor + 4 <= slice.len() {
            let index = u16::from_le_bytes([slice[cursor], slice[cursor + 1]]) as usize;
            cursor += 2;
            let len = u16::from_le_bytes([slice[cursor], slice[cursor + 1]]) as usize;
            cursor += 2;

            if cursor + len > slice.len() {
                break;
            }

            let u16_chars: Vec<u16> = slice[cursor..cursor + len]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let py = String::from_utf16_lossy(&u16_chars);
            cursor += len;

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
    }

    #[test]
    fn test_parse_plain_text() {
        let sample = "测试 ce'shi 100\n输入法 shu'ru'fa 200\n";
        let res = DictImporter::parse_plain_text(sample).unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].text, "测试");
        assert_eq!(res[1].text, "输入法");
    }
}
