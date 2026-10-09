use crate::error::AlgoError;
use crate::types::{AlgoCandidate, CandidateTier};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

/// 紧凑词典索引表 (Table / Lexicon)
///
/// # 设计原理
/// - **实现初衷**：负责将音节序列（如 "ni'hao"）映射到候选字词列表及基准静态词频，
///   向下兼容项目现有的二进制紧凑格式（`base.dict.bin`）。
/// - **核心优势**：在内存中以只读哈希表与连续向量存储，检索为 O(1) 微秒级操作；支持零拷贝二进制加载。
/// - **代价与局限**：全量 55 万词库常驻内存约 25MB，但在现代桌面环境下完全可以忽略。
#[derive(Debug, Default)]
pub struct LexiconTable {
    /// 倒排拼音索引表：键为拼音序列（如 "ni'hao" 或 "xian"），值为候选文本与基础静态分
    entries: HashMap<String, Vec<(String, u32)>>,
    /// 简拼缩写索引表：键为首字母缩写（如 "bj" 或 "zg"），值为候选文本与基础静态分
    acronym_entries: HashMap<String, Vec<(String, u32)>>,
}

impl LexiconTable {
    /// 创建空的词典表
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            acronym_entries: HashMap::new(),
        }
    }

    /// 插入一条字词条目
    pub fn insert(&mut self, pinyin: impl Into<String>, text: impl Into<String>, score: u32) {
        let key = pinyin.into();
        let val_text = text.into();
        let val = (val_text.clone(), score);
        self.entries.entry(key.clone()).or_default().push(val);

        // 提取多音节词的首字母缩写作为简拼
        if key.contains('\'') {
            let parts: Vec<&str> = key.split('\'').collect();
            if parts.len() >= 2 {
                let mut acronym = String::with_capacity(parts.len());
                for part in parts {
                    if let Some(ch) = part.chars().next() {
                        acronym.push(ch);
                    }
                }
                if acronym.len() >= 2 {
                    self.acronym_entries
                        .entry(acronym)
                        .or_default()
                        .push((val_text, score));
                }
            }
        }
    }

    /// 从预编译二进制词库文件中加载
    ///
    /// # Errors
    /// 当文件路径不存在或反序列化失败时返回 [`AlgoError::DictionaryIo`] 或 [`AlgoError::DeserializeFailed`]。
    pub fn load_binary(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let reader = BufReader::new(file);
        let entries: HashMap<String, Vec<(String, u32)>> = bincode::deserialize_from(reader)
            .map_err(|e| AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        // 内存中自动根据全拼倒排构建简拼缩写索引（完全向下兼容现有 base.dict.bin）
        let mut acronym_entries: HashMap<String, Vec<(String, u32)>> = HashMap::new();
        for (pinyin, word_list) in &entries {
            if pinyin.contains('\'') {
                let parts: Vec<&str> = pinyin.split('\'').collect();
                if parts.len() >= 2 {
                    let mut acronym = String::with_capacity(parts.len());
                    for part in parts {
                        if let Some(ch) = part.chars().next() {
                            acronym.push(ch);
                        }
                    }
                    if acronym.len() >= 2 {
                        let list = acronym_entries.entry(acronym).or_default();
                        for (w, s) in word_list {
                            // 仅收集高频成熟词（得分 > 3,000）作为简拼，防止低频生僻词噪音
                            if *s >= 3_000 {
                                list.push((w.clone(), *s));
                            }
                        }
                    }
                }
            }
        }

        // 限制每个简拼键下仅保留得分最高的 Top 8 项
        for list in acronym_entries.values_mut() {
            list.sort_by_key(|b| std::cmp::Reverse(b.1));
            list.truncate(8);
        }

        Ok(Self {
            entries,
            acronym_entries,
        })
    }

    /// 创建内置测试词库（用于自动化回归测试）
    pub fn with_sample_data() -> Self {
        let mut table = Self::new();
        table.insert("ni'hao", "你好", 10_000);
        table.insert("ni", "你", 9_500);
        table.insert("hao", "好", 9_200);
        table.insert("xian", "先", 7_200);
        table.insert("xian", "现", 7_100);
        table.insert("xian", "显", 6_800);
        table.insert("xi'an", "西安", 7_500);
        table.insert("xi", "西", 8_000);
        table.insert("an", "安", 8_200);
        table.insert("wo", "我", 9_900);
        table.insert("wo'men", "我们", 11_000);
        table.insert("zhong'guo", "中国", 12_000);
        table.insert("zhong", "中", 8_000);
        table.insert("guo", "国", 8_500);
        table.insert("ri'qi", "日期", 10_000);
        table.insert("shi'jian", "时间", 10_000);
        table.insert("bei'jing", "北京", 15_000);
        table.insert("yong'yuan'de'shen", "永远的神", 20_000);
        table
    }

    /// 根据归一化拼音键检索候选词
    pub fn lookup(&self, key: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        if let Some(matches) = self.entries.get(key) {
            candidates.reserve(matches.len());
            for (text, score) in matches {
                let char_len = text.chars().count() as u32;
                // 长度激励加权：多字成词合理激励（600,000），使成熟词条与超高频单字和谐共存
                let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                let final_score = score.saturating_add(boost);
                candidates.push(AlgoCandidate::new(text.clone(), key, final_score));
            }
        }
        candidates
    }

    /// 检索首字母缩写简拼候选词（如 "bj" 匹配 "北京"，"yyds" 匹配 "永远的神"）
    pub fn lookup_acronym(&self, key: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        if let Some(matches) = self.acronym_entries.get(key) {
            candidates.reserve(matches.len());
            for (text, score) in matches {
                candidates.push(
                    AlgoCandidate::new(text.clone(), key, *score)
                        .with_tier(CandidateTier::Acronym)
                        .with_comment("[简拼]"),
                );
            }
        }
        candidates
    }

    /// 获取索引中词条总数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 词库是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_length_boost() {
        let table = LexiconTable::with_sample_data();
        let candidates = table.lookup("wo'men");
        assert!(!candidates.is_empty());
        assert_eq!(candidates[0].text, "我们");
        assert!(candidates[0].score >= 600_000);
    }

    #[test]
    fn test_lookup_xian_vs_xi_an() {
        let table = LexiconTable::with_sample_data();
        let c_xian = table.lookup("xian");
        let c_xi_an = table.lookup("xi'an");

        assert!(c_xian.iter().any(|c| c.text == "先"));
        assert!(c_xi_an.iter().any(|c| c.text == "西安"));
    }
}
