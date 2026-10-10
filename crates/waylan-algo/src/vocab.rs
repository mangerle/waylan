use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use crate::error::AlgoError;

/// 微澜全局词表魔数标识（16 字节）
pub const VOCAB_MAGIC: &[u8; 16] = b"WAYLAN_VOCAB01\0\0";

/// 统一全局词表管理器 (Vocabulary)
///
/// # 设计原理
/// - **实现初衷**：为二元及高阶语言模型提供确定性的整数 Word ID 映射，替代冗长的 64 位字符串哈希；
/// - **核心优势**：
///   1. 双向高效转换：支持 $O(1)$ 文本查 ID 与 $O(1)$ 数组下标直接反查文本；
///   2. 确定性编码：词表一旦编译固化，全局 ID 终身稳定，确保二进制 CSR 矩阵的高效寻址与版本互通；
///   3. 零内存碎片：支持紧凑二进制持久化与极速读取。
#[derive(Debug, Clone, Default)]
pub struct Vocabulary {
    /// 词条数组列表（下标即为 Word ID）
    words: Vec<String>,
    /// 词条到 Word ID 的倒排映射表
    word_to_id: HashMap<String, u32>,
}

impl Vocabulary {
    /// 创建空的词表
    pub fn new() -> Self {
        Self {
            words: Vec::new(),
            word_to_id: HashMap::new(),
        }
    }

    /// 获取词表中收录的总词条数
    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// 判断词表是否为空
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// 插入新词条，若已存在则返回已有 Word ID，若不存在则分配新的连续 ID
    pub fn insert_or_get(&mut self, word: &str) -> u32 {
        if let Some(&id) = self.word_to_id.get(word) {
            return id;
        }

        let new_id = self.words.len() as u32;
        let s = word.to_string();
        self.words.push(s.clone());
        self.word_to_id.insert(s, new_id);
        new_id
    }

    /// 查询词条对应的 Word ID
    #[inline]
    pub fn get_id(&self, word: &str) -> Option<u32> {
        self.word_to_id.get(word).copied()
    }

    /// 根据 Word ID 反查对应的词条文本
    #[inline]
    pub fn get_word(&self, id: u32) -> Option<&str> {
        self.words.get(id as usize).map(|s| s.as_str())
    }

    /// 获取所有词条切片
    pub fn all_words(&self) -> &[String] {
        &self.words
    }

    /// 从微澜标准拼音词典 YAML 文件集合中解析提取基础词表
    pub fn load_from_dict_yamls<P: AsRef<Path>>(paths: &[P]) -> Result<Self, AlgoError> {
        let mut vocab = Self::new();

        for p in paths {
            let path_ref = p.as_ref();
            let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
            let reader = BufReader::new(file);

            for line_res in reader.lines() {
                let line = match line_res {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("---") || trimmed.starts_with("...") {
                    continue;
                }

                let parts: Vec<&str> = trimmed.split('\t').collect();
                if parts.len() >= 2 {
                    let text = parts[0].trim();
                    if !text.is_empty() {
                        vocab.insert_or_get(text);
                    }
                }
            }
        }

        Ok(vocab)
    }

    /// 保存词表到紧凑二进制文件
    ///
    /// # 二进制结构
    /// - Header: 16B 魔数 + 4B 版本号(1) + 4B 词表容量 V
    /// - String Table: 连续紧凑 UTF-8 字符串段（每个词以 `u16 长度 + 字节流` 存储）
    pub fn save_binary(&self, path: impl AsRef<Path>) -> Result<(), AlgoError> {
        let path_ref = path.as_ref();
        let file = File::create(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let mut writer = BufWriter::new(file);

        // 1. 写入魔数与头部
        writer.write_all(VOCAB_MAGIC).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        writer.write_all(&1u32.to_le_bytes()).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        writer.write_all(&(self.words.len() as u32).to_le_bytes()).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        // 2. 依次写入各词条
        for word in &self.words {
            let bytes = word.as_bytes();
            let len = bytes.len() as u16;
            writer.write_all(&len.to_le_bytes()).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
            writer.write_all(bytes).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
        }

        writer.flush().map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        Ok(())
    }

    /// 从紧凑二进制文件载入词表
    pub fn load_binary(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let mut reader = BufReader::new(file);

        let mut magic = [0u8; 16];
        reader.read_exact(&mut magic).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        if &magic != VOCAB_MAGIC {
            return Err(AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: bincode::ErrorKind::Custom("词表文件魔数不匹配".to_string()).into(),
            });
        }

        let mut ver_bytes = [0u8; 4];
        reader.read_exact(&mut ver_bytes).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let _version = u32::from_le_bytes(ver_bytes);

        let mut count_bytes = [0u8; 4];
        reader.read_exact(&mut count_bytes).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let count = u32::from_le_bytes(count_bytes) as usize;

        let mut words = Vec::with_capacity(count);
        let mut word_to_id = HashMap::with_capacity(count);
        let mut buf = Vec::with_capacity(64);

        for id in 0..count {
            let mut len_bytes = [0u8; 2];
            reader.read_exact(&mut len_bytes).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
            let len = u16::from_le_bytes(len_bytes) as usize;

            buf.resize(len, 0);
            reader.read_exact(&mut buf).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

            let s = std::str::from_utf8(&buf).map_err(|e| AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: bincode::ErrorKind::Custom(format!("词条UTF8解析异常: {}", e)).into(),
            })?.to_string();

            word_to_id.insert(s.clone(), id as u32);
            words.push(s);
        }

        Ok(Self { words, word_to_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vocab_insert_and_lookup() {
        let mut vocab = Vocabulary::new();
        let id0 = vocab.insert_or_get("你好");
        let id1 = vocab.insert_or_get("世界");
        let id0_again = vocab.insert_or_get("你好");

        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(id0_again, 0);
        assert_eq!(vocab.len(), 2);

        assert_eq!(vocab.get_id("你好"), Some(0));
        assert_eq!(vocab.get_id("世界"), Some(1));
        assert_eq!(vocab.get_id("未知"), None);

        assert_eq!(vocab.get_word(0), Some("你好"));
        assert_eq!(vocab.get_word(1), Some("世界"));
        assert_eq!(vocab.get_word(2), None);
    }

    #[test]
    fn test_vocab_save_and_load_roundtrip() {
        let mut vocab = Vocabulary::new();
        vocab.insert_or_get("北京");
        vocab.insert_or_get("时间");
        vocab.insert_or_get("清风拂面");

        let tmp_path = std::env::temp_dir().join("test_waylan_vocab.bin");
        vocab.save_binary(&tmp_path).expect("保存词表成功");

        let loaded = Vocabulary::load_binary(&tmp_path).expect("载入词表成功");
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded.get_id("北京"), Some(0));
        assert_eq!(loaded.get_id("时间"), Some(1));
        assert_eq!(loaded.get_id("清风拂面"), Some(2));
        assert_eq!(loaded.get_word(2), Some("清风拂面"));

        let _ = std::fs::remove_file(tmp_path);
    }
}
