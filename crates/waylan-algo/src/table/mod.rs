use crate::bigram::CompactMmapBigram;
use crate::error::AlgoError;
use crate::prism::STANDARD_INITIALS;
use crate::types::AlgoCandidate;
use crate::vocab::Vocabulary;
use std::collections::HashMap;
use std::path::Path;

pub mod mmap;
mod prefix;
mod sample;

pub use mmap::{CompactMmapLexicon, COMPACT_DICT_MAGIC};

/// 紧凑词典索引表 (Table / Lexicon)
///
/// # 设计原理
/// - 统一管理拼音检索、简拼检索、声母检索与 Bigram 语言模型转移亲和度；
/// - 原生挂载 `CompactMmapLexicon` (DAT 一体化)，支持零反序列化微秒级加载；
/// - 兼容动态用户词表与单元测试内置字典。
#[derive(Debug)]
pub struct LexiconTable {
    /// 倒排拼音索引表：键为拼音序列（如 "ni'hao"），值为候选文本与基础静态分
    entries: HashMap<String, Vec<(String, u32)>>,
    /// 简拼缩写索引表：键为首字母缩写（如 "bj"），值为候选文本与基础静态分
    pub(crate) acronym_entries: HashMap<String, Vec<(String, u32)>>,
    /// 标准声母高频单字索引表（如 "s", "d", "g"）
    pub(crate) initial_entries: HashMap<String, Vec<(String, u32)>>,
    /// 零拷贝紧凑只读词典（DAT + 单源只读词表）
    pub compact_mmap: Option<CompactMmapLexicon>,
    /// 零拷贝紧凑只读转移概率矩阵 (CSR 架构)
    pub compact_bigram: Option<CompactMmapBigram>,
    /// 向后兼容的内存词表（仅在无底库文件时作为纯内存回退）
    pub vocab: Option<Vocabulary>,
}

impl Default for LexiconTable {
    fn default() -> Self {
        Self::new()
    }
}

impl LexiconTable {
    /// 创建空的词典表
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            acronym_entries: HashMap::new(),
            initial_entries: HashMap::new(),
            compact_mmap: None,
            compact_bigram: None,
            vocab: None,
        }
    }

    /// 向候选词列表中插入或更新词条（相同词文本保留最高分，并保持按得分降序排列）
    pub(crate) fn upsert_entry(list: &mut Vec<(String, u32)>, text: String, score: u32) {
        if let Some(existing) = list.iter_mut().find(|(t, _)| *t == text) {
            if score > existing.1 {
                existing.1 = score;
            }
        } else {
            list.push((text, score));
        }
        list.sort_by_key(|b| std::cmp::Reverse(b.1));
    }

    /// 插入一条字词条目（内存动态词库）
    pub fn insert(&mut self, pinyin: impl Into<String>, text: impl Into<String>, score: u32) {
        let key = pinyin.into();
        let val_text = text.into();
        let entry_list = self.entries.entry(key.clone()).or_default();
        Self::upsert_entry(entry_list, val_text.clone(), score);

        // 维护声母高频单字索引
        if !key.contains('\'') && val_text.chars().count() == 1 {
            for &initial in STANDARD_INITIALS {
                if key.starts_with(initial) {
                    let list = self.initial_entries.entry(initial.to_string()).or_default();
                    Self::upsert_entry(list, val_text.clone(), score);
                    list.truncate(5);
                    break;
                }
            }
        }

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
                    let list = self.acronym_entries.entry(acronym).or_default();
                    Self::upsert_entry(list, val_text, score);
                }
            }
        }
    }

    /// 从预编译二进制紧凑词库文件中加载（零拷贝内存映射）
    ///
    /// 自动校验 DAT 一体化格式（魔数 `WAYLAN_COMPACT02`），并自动探测挂载同目录下的 `bigram.table.bin`。
    ///
    /// # Errors
    /// 当词库文件不存在、读取失败或格式魔数/头部校验不通过时返回 [`AlgoError`]。
    pub fn load_binary(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let compact = CompactMmapLexicon::open(path_ref)?;

        // 自动探测并挂载 CSR 高密度转移概率矩阵 (Word ID 体系已一体化)
        let bigram_path = path_ref.with_file_name("bigram.table.bin");
        let compact_bigram = if bigram_path.exists() {
            CompactMmapBigram::open(&bigram_path).ok()
        } else {
            None
        };

        Ok(Self {
            entries: HashMap::new(),
            acronym_entries: HashMap::new(),
            initial_entries: HashMap::new(),
            compact_mmap: Some(compact),
            compact_bigram,
            vocab: None,
        })
    }

    /// 反查词条的唯一整数 ID（纳秒级二分反查或哈希表索引）
    #[inline]
    pub fn get_word_id(&self, word: &str) -> Option<u32> {
        if let Some(compact) = &self.compact_mmap {
            if let Some(id) = compact.get_id(word) {
                return Some(id);
            }
        }
        if let Some(vocab) = &self.vocab {
            if let Some(id) = vocab.get_id(word) {
                return Some(id);
            }
        }
        None
    }

    /// 直接根据两个词的唯一 ID 查询二元转移亲和度得分（零字符串查找，微秒级 CSR 查找）
    #[inline]
    pub fn get_bigram_affinity_by_ids(&self, prev_id: u32, curr_id: u32) -> Option<i64> {
        if let Some(bg) = &self.compact_bigram {
            return bg.get_affinity_by_id(prev_id, curr_id);
        }
        None
    }

    /// 已知前驱词 ID 时直接查询二元词对亲和度得分（避免在候选循环内重复解析前驱词 ID）
    #[inline]
    pub fn get_bigram_affinity_by_first_id(&self, prev_id: u32, curr: &str) -> Option<i64> {
        if let Some(bg) = &self.compact_bigram {
            if let Some(curr_id) = self.get_word_id(curr) {
                return bg.get_affinity_by_id(prev_id, curr_id);
            }
        }
        None
    }

    /// 查询二元词对统计转移亲和度加权得分
    #[inline]
    pub fn get_bigram_affinity(&self, prev: &str, curr: &str) -> Option<i64> {
        let prev_id = self.get_word_id(prev)?;
        self.get_bigram_affinity_by_first_id(prev_id, curr)
    }

    /// 根据归一化拼音键检索候选词
    pub fn lookup(&self, key: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();

        // 1. 若挂载了零拷贝 DAT 一体化紧凑词库，直接状态跳转检索
        if let Some(compact) = &self.compact_mmap {
            if let Some(matches) = compact.lookup(key) {
                candidates.reserve(matches.len());
                for (text, score, word_id) in matches {
                    let char_len = text.chars().count() as u32;
                    let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                    let final_score = score.saturating_add(boost);
                    candidates.push(
                        AlgoCandidate::new(text, key, final_score).with_word_id(Some(word_id)),
                    );
                }
            }
        }

        // 2. 检索内存动态词表
        if let Some(matches) = self.entries.get(key) {
            candidates.reserve(matches.len());
            for (text, score) in matches {
                let char_len = text.chars().count() as u32;
                let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                let final_score = score.saturating_add(boost);
                candidates.push(AlgoCandidate::new(text.clone(), key, final_score));
            }
        }

        candidates
    }

    /// 根据归一化拼音键检索按需截取 Top-K 候选词（避免全量同音词解码与分配）
    pub fn lookup_top_k(&self, key: &str, k: usize) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::with_capacity(k);

        // 1. 若挂载了零拷贝 DAT 一体化紧凑词库，按需获取 Top-K
        if let Some(compact) = &self.compact_mmap {
            if let Some(matches) = compact.lookup_top_k(key, k) {
                candidates.reserve(matches.len());
                for (text, score, word_id) in matches {
                    let char_len = text.chars().count() as u32;
                    let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                    let final_score = score.saturating_add(boost);
                    candidates.push(
                        AlgoCandidate::new(text, key, final_score).with_word_id(Some(word_id)),
                    );
                }
            }
        }

        // 2. 检索内存动态词表
        if let Some(matches) = self.entries.get(key) {
            for (text, score) in matches.iter().take(k) {
                let char_len = text.chars().count() as u32;
                let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                let final_score = score.saturating_add(boost);
                candidates.push(AlgoCandidate::new(text.clone(), key, final_score));
            }
        }

        candidates.truncate(k);
        candidates
    }

    /// 获取指定拼音键的原始词库条目（合并紧凑词库与内存增量词表）
    pub fn get_raw_entries(&self, key: &str) -> Option<Vec<(String, u32)>> {
        let compact_res = self.compact_mmap.as_ref().and_then(|c| c.lookup(key));
        let mem_res = self.entries.get(key);

        match (compact_res, mem_res) {
            (Some(c_list), Some(m_list)) => {
                let mut res: Vec<(String, u32)> =
                    c_list.into_iter().map(|(t, s, _)| (t, s)).collect();
                for (text, score) in m_list {
                    Self::upsert_entry(&mut res, text.clone(), *score);
                }
                Some(res)
            }
            (Some(c_list), None) => Some(c_list.into_iter().map(|(t, s, _)| (t, s)).collect()),
            (None, Some(m_list)) => Some(m_list.clone()),
            (None, None) => None,
        }
    }

    /// 按需获取指定拼音键的前 `k` 条高频原始词条（合并紧凑词库与内存增量词表并截取 Top-K，保留 word_id）
    pub fn get_top_raw_entries(
        &self,
        key: &str,
        k: usize,
    ) -> Option<Vec<(String, u32, Option<u32>)>> {
        let compact_res = self
            .compact_mmap
            .as_ref()
            .and_then(|c| c.lookup_top_k(key, k));
        let mem_res = self.entries.get(key);

        match (compact_res, mem_res) {
            (Some(c_list), Some(m_list)) => {
                let mut res: Vec<(String, u32, Option<u32>)> = c_list
                    .into_iter()
                    .map(|(t, s, wid)| (t, s, Some(wid)))
                    .collect();
                for (text, score) in m_list.iter().take(k) {
                    if let Some(existing) = res.iter_mut().find(|(t, _, _)| t == text) {
                        if *score > existing.1 {
                            existing.1 = *score;
                        }
                    } else {
                        res.push((text.clone(), *score, None));
                    }
                }
                res.sort_by_key(|b| std::cmp::Reverse(b.1));
                res.truncate(k);
                Some(res)
            }
            (Some(c_list), None) => Some(
                c_list
                    .into_iter()
                    .map(|(t, s, wid)| (t, s, Some(wid)))
                    .collect(),
            ),
            (None, Some(m_list)) => {
                let mut top: Vec<(String, u32, Option<u32>)> = m_list
                    .iter()
                    .take(k)
                    .map(|(t, s)| (t.clone(), *s, None))
                    .collect();
                if top.len() > 1 {
                    top.sort_by_key(|b| std::cmp::Reverse(b.1));
                }
                Some(top)
            }
            (None, None) => None,
        }
    }

    /// 获取索引中词条总数
    pub fn len(&self) -> usize {
        let compact_len = self.compact_mmap.as_ref().map(|c| c.num_keys).unwrap_or(0);
        compact_len + self.entries.len()
    }

    /// 词库是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_deduplication_and_descending_order() {
        let mut table = LexiconTable::new();
        table.insert("shi'jian", "时间", 10_000);
        table.insert("shi'jian", "事件", 15_000);
        table.insert("shi'jian", "时间", 20_000); // 同词更高分应更新并排在首位
        table.insert("shi'jian", "事件", 5_000); // 同词更低分应忽略

        let raw = table.get_raw_entries("shi'jian").expect("应存在词条");
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0], ("时间".to_string(), 20_000));
        assert_eq!(raw[1], ("事件".to_string(), 15_000));
    }
}
