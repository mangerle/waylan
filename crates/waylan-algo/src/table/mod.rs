use crate::bigram::CompactMmapBigram;
use crate::compact_writer::compile_entries_to_compact_bytes;
use crate::error::AlgoError;
use crate::types::AlgoCandidate;
use std::collections::HashMap;
use std::path::Path;

pub mod mmap;
mod mmap_header;
mod prefix;
mod sample;

pub use mmap::CompactMmapLexicon;
pub use mmap_header::COMPACT_DICT_MAGIC;

/// 词长加权分基数（每增加一字长度提权 600,000 分，确保长词优先于单字短词）
pub const LENGTH_BOOST_MULTIPLIER: u32 = 600_000;

/// 计算词长带来的提权加分
///
/// # 设计原理
/// - 单字（长度 <= 1）无提权加分；
/// - 多字词每增加一字，获得 `LENGTH_BOOST_MULTIPLIER` 权重的提权加分；
/// - 采用饱和算术防范潜在溢出。
#[inline]
pub fn calculate_length_boost(char_count: u32) -> u32 {
    char_count
        .saturating_sub(1)
        .saturating_mul(LENGTH_BOOST_MULTIPLIER)
}

/// 剥离词长加权分，还原原始词库静态分
#[inline]
pub fn extract_raw_score(boosted_score: u32) -> u32 {
    boosted_score % LENGTH_BOOST_MULTIPLIER
}

/// 紧凑词典索引表 (Table / Lexicon)
///
/// # 设计原理
/// - 统一管理拼音检索、简拼检索、声母检索与 Bigram 语言模型转移亲和度；
/// - 原生挂载 `CompactMmapLexicon` (DAT 一体化)，支持零反序列化微秒级加载；
/// - 兼容动态用户词表与单元测试内置字典。
#[derive(Debug)]
pub struct LexiconTable {
    /// 动态内存构建缓冲（仅在通过 `insert` 增量构建测试 fixture 时使用，查询始终走 DAT）
    pub(crate) dynamic_entries: HashMap<String, Vec<(String, u32)>>,
    /// 零拷贝紧凑只读词典（DAT + 单源只读词表）
    pub compact_mmap: Option<CompactMmapLexicon>,
    /// 零拷贝紧凑只读转移概率矩阵 (CSR 架构)
    pub compact_bigram: Option<CompactMmapBigram>,
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
            dynamic_entries: HashMap::new(),
            compact_mmap: None,
            compact_bigram: None,
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

    /// 插入一条字词条目并即时重编译内存 DAT 视图（用于动态测试 fixture 构建）
    pub fn insert(&mut self, pinyin: impl Into<String>, text: impl Into<String>, score: u32) {
        let key = pinyin.into();
        let val_text = text.into();
        let entry_list = self.dynamic_entries.entry(key).or_default();
        Self::upsert_entry(entry_list, val_text, score);

        let bytes = compile_entries_to_compact_bytes(&self.dynamic_entries);
        if let Ok(compact) = CompactMmapLexicon::from_bytes(&bytes) {
            self.compact_mmap = Some(compact);
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
            dynamic_entries: HashMap::new(),
            compact_mmap: Some(compact),
            compact_bigram,
        })
    }

    /// 反查词条的唯一整数 ID（纳秒级二分反查）
    #[inline]
    pub fn get_word_id(&self, word: &str) -> Option<u32> {
        self.compact_mmap.as_ref()?.get_id(word)
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
        self.lookup_into(key, &mut candidates);
        candidates
    }

    /// 根据归一化拼音键检索候选词并写入指定的复用容器中（避免堆重新分配）
    pub fn lookup_into(&self, key: &str, out: &mut Vec<AlgoCandidate>) {
        if let Some(compact) = &self.compact_mmap {
            if let Some(matches) = compact.lookup(key) {
                out.reserve(matches.len());
                for (text, score, word_id) in matches {
                    let char_len = text.chars().count() as u32;
                    let boost = calculate_length_boost(char_len);
                    let final_score = score.saturating_add(boost);
                    out.push(
                        AlgoCandidate::new(text, key, final_score).with_word_id(Some(word_id)),
                    );
                }
            }
        }
    }

    /// 根据归一化拼音键检索按需截取 Top-K 候选词（避免全量同音词解码与分配）
    pub fn lookup_top_k(&self, key: &str, k: usize) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::with_capacity(k);
        self.lookup_top_k_into(key, k, &mut candidates);
        candidates
    }

    /// 根据归一化拼音键检索按需截取 Top-K 候选词并写入指定的复用容器中（避免堆重新分配）
    pub fn lookup_top_k_into(&self, key: &str, k: usize, out: &mut Vec<AlgoCandidate>) {
        if let Some(compact) = &self.compact_mmap {
            if let Some(matches) = compact.lookup_top_k(key, k) {
                out.reserve(matches.len());
                for (text, score, word_id) in matches {
                    let char_len = text.chars().count() as u32;
                    let boost = calculate_length_boost(char_len);
                    let final_score = score.saturating_add(boost);
                    out.push(
                        AlgoCandidate::new(text, key, final_score).with_word_id(Some(word_id)),
                    );
                }
            }
        }
    }

    /// 获取指定拼音键的原始词库条目
    pub fn get_raw_entries(&self, key: &str) -> Option<Vec<(String, u32)>> {
        self.compact_mmap
            .as_ref()?
            .lookup(key)
            .map(|list| list.into_iter().map(|(t, s, _)| (t, s)).collect())
    }

    /// 按需获取指定拼音键的前 `k` 条高频原始词条（保留 word_id）
    pub fn get_top_raw_entries(
        &self,
        key: &str,
        k: usize,
    ) -> Option<Vec<(String, u32, Option<u32>)>> {
        self.compact_mmap
            .as_ref()?
            .lookup_top_k(key, k)
            .map(|list| {
                list.into_iter()
                    .map(|(t, s, wid)| (t, s, Some(wid)))
                    .collect()
            })
    }

    /// 获取索引中词条总数
    pub fn len(&self) -> usize {
        self.compact_mmap.as_ref().map(|c| c.num_keys).unwrap_or(0)
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
