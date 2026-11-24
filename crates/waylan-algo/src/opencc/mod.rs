//! 简繁转换与正向最大匹配消歧模块
//!
//! 整合 OpenCC 字符短语字典、正向最大匹配（FMM）分词转换与紧凑二进制持久化。

pub mod compact;
mod converter;
mod data;
mod storage;
pub mod tsv;

use self::compact::CompactPhraseTable;
use crate::error::AlgoError;
use log::info;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
pub use tsv::OpenCcTsvParser;

/// 简繁转换二进制词库魔数标识（16 字节）
pub const OPENCC_DICT_MAGIC: &[u8; 16] = b"WAYLAN_OPENCC001";

/// 全局共享默认简繁转换词典缓存
static GLOBAL_OPENCC_DICT: OnceLock<Arc<OpenCcDict>> = OnceLock::new();

/// 紧凑短语映射表（按 UTF-8 字节序连续存储，通过二分查找实现零堆分配极速查询）
#[derive(Debug, Clone, Default)]
pub struct OpenCcDict {
    /// 简化字单字 -> 繁体字序列（首元素为默认首选繁体字，后续元素为一简多繁备选字）
    pub chars: HashMap<char, Vec<char>>,
    /// 简体短语 -> 繁体短语映射表（动态构造与 TSV 构建阶段使用）
    pub phrases: HashMap<String, String>,
    /// 紧凑短语表（从二进制词库加载时常驻，提供零堆分配二分检索）
    pub compact_phrases: Option<CompactPhraseTable>,
    /// 短语首字 -> 以该字开头的最长短语字符数（用于加速 FMM 剪枝）
    pub first_char_max_len: HashMap<char, usize>,
}

impl OpenCcDict {
    /// 创建空的简繁转换词典
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取短语总条目数
    pub fn phrases_len(&self) -> usize {
        if let Some(ref compact) = self.compact_phrases {
            compact.len()
        } else {
            self.phrases.len()
        }
    }

    /// 查询指定简体短语对应的繁体短语（优先二分检索紧凑表，回退查询 HashMap）
    pub fn lookup_phrase(&self, simp: &str) -> Option<&str> {
        if let Some(ref compact) = self.compact_phrases {
            return compact.lookup(simp);
        }
        self.phrases.get(simp).map(|s| s.as_str())
    }

    /// 获取全局共享简繁转换词典实例（优先加载二进制 opencc.dict.bin，其次加载开源词表文本，最后使用内置核心表兜底）
    pub fn global() -> Arc<Self> {
        GLOBAL_OPENCC_DICT
            .get_or_init(|| Arc::new(Self::load_default()))
            .clone()
    }

    /// 按多级路径加载默认简繁词典
    pub fn load_default() -> Self {
        let bin_candidates = waylan_core::config::opencc_dict_search_paths();
        Self::load_from_search_paths(&bin_candidates)
    }

    /// 从显式传入的二进制词典候选路径列表加载简繁词典，均未命中时回退至内置 TSV 或核心表
    pub fn load_from_search_paths(bin_candidates: &[PathBuf]) -> Self {
        for path in bin_candidates {
            if path.exists()
                && let Ok(dict) = Self::load_from_file(path)
            {
                info!(
                    "[简繁词库] 成功载入二进制简繁转换词库: {:?} (单字映射: {} 个, 消歧短语: {} 组)",
                    path,
                    dict.chars.len(),
                    dict.phrases_len()
                );
                return dict;
            }
        }

        let tsv_dirs = [
            PathBuf::from("data/dicts/8_opencc"),
            PathBuf::from("../../data/dicts/8_opencc"),
            PathBuf::from("/usr/share/waylan/dicts/8_opencc"),
        ];
        for dir in &tsv_dirs {
            let chars_path = dir.join("STCharacters.txt");
            let phrases_path = dir.join("STPhrases.txt");
            if chars_path.exists()
                && phrases_path.exists()
                && let Ok(dict) = Self::from_tsv_files(&chars_path, &phrases_path)
            {
                info!("[简繁词库] 成功从 TSV 源文件载入简繁转换词库: {:?}", dir);
                return dict;
            }
        }

        info!("[简繁词库] 未检测到外部词库文件，启用内置核心简繁映射兜底表。");
        Self::with_builtin_fallback()
    }

    /// 内置核心简繁单字与一简多繁短语兜底表（供无外部词库文件时的轻量环境使用）
    pub fn with_builtin_fallback() -> Self {
        let mut dict = Self::new();
        for &(simp, trads) in self::data::BUILTIN_CHARS {
            dict.insert_char_mapping(simp, trads.to_vec());
        }
        for &(simp, trad) in self::data::BUILTIN_PHRASES {
            dict.insert_phrase_mapping(simp.to_string(), trad.to_string());
        }

        dict
    }

    /// 插入一条单字简繁映射
    pub fn insert_char_mapping(&mut self, simp: char, trads: Vec<char>) {
        if !trads.is_empty() {
            self.chars.insert(simp, trads);
        }
    }

    /// 插入一条短语简繁映射并更新首字最大词长索引
    pub fn insert_phrase_mapping(&mut self, simp: String, trad: String) {
        let mut char_iter = simp.chars();
        if let Some(first_ch) = char_iter.next() {
            let char_len = 1 + char_iter.count();
            if char_len >= 2 {
                let entry = self.first_char_max_len.entry(first_ch).or_insert(0);
                if char_len > *entry {
                    *entry = char_len;
                }
                self.phrases.insert(simp, trad);
            }
        }
    }

    /// 从 OpenCC 标准 TSV 文本文件解析构建词典
    #[inline]
    pub fn from_tsv_files(
        chars_path: impl AsRef<Path>,
        phrases_path: impl AsRef<Path>,
    ) -> Result<Self, AlgoError> {
        self::tsv::OpenCcTsvParser::from_tsv_files(chars_path, phrases_path)
    }

    /// 解析 `STCharacters.txt` 格式内容
    #[inline]
    pub fn parse_characters_tsv(&mut self, content: &str) {
        self::tsv::OpenCcTsvParser::parse_characters_tsv(self, content);
    }

    /// 解析 `STPhrases.txt` 格式内容
    #[inline]
    pub fn parse_phrases_tsv(&mut self, content: &str) {
        self::tsv::OpenCcTsvParser::parse_phrases_tsv(self, content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opencc_builtin_fallback_conversion() {
        let dict = OpenCcDict::with_builtin_fallback();
        assert!(!dict.chars.is_empty());
        assert!(!dict.phrases.is_empty());

        let converted = dict.convert_text("发展自然语言处理");
        assert!(converted.contains('展') || converted.contains('展'));
    }
}
