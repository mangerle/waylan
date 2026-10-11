use crate::bigram::CompactMmapBigram;
use crate::error::AlgoError;
use crate::prism::STANDARD_INITIALS;
use crate::types::AlgoCandidate;
use crate::vocab::Vocabulary;
use std::collections::HashMap;
use std::path::Path;

pub use crate::table_mmap::{CompactMmapLexicon, COMPACT_DICT_MAGIC};

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

    /// 插入一条字词条目（内存动态词库）
    pub fn insert(&mut self, pinyin: impl Into<String>, text: impl Into<String>, score: u32) {
        let key = pinyin.into();
        let val_text = text.into();
        let val = (val_text.clone(), score);
        self.entries.entry(key.clone()).or_default().push(val);

        // 维护声母高频单字索引
        if !key.contains('\'') && val_text.chars().count() == 1 {
            for &initial in STANDARD_INITIALS {
                if key.starts_with(initial) {
                    let list = self.initial_entries.entry(initial.to_string()).or_default();
                    list.push((val_text.clone(), score));
                    list.sort_by_key(|b| std::cmp::Reverse(b.1));
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
                    self.acronym_entries
                        .entry(acronym)
                        .or_default()
                        .push((val_text, score));
                }
            }
        }
    }

    /// 从预编译二进制紧凑词库文件中加载（零拷贝内存映射）
    ///
    /// 自动校验 DAT 一体化格式（魔数 `WAYLAN_COMPACT02`），并自动探测挂载同目录下的 `bigram.table.bin`。
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

    /// 查询二元词对统计转移亲和度加权得分
    #[inline]
    pub fn get_bigram_affinity(&self, prev: &str, curr: &str) -> Option<i64> {
        if let Some(bg) = &self.compact_bigram {
            // 优先从一体化底库快速反查 Word ID (纳秒级二分，零堆开销)
            if let Some(compact) = &self.compact_mmap {
                if let (Some(id1), Some(id2)) = (compact.get_id(prev), compact.get_id(curr)) {
                    return bg.get_affinity_by_id(id1, id2);
                }
            }
            // 回退到动态内存词表
            if let Some(vocab) = &self.vocab {
                if let (Some(id1), Some(id2)) = (vocab.get_id(prev), vocab.get_id(curr)) {
                    return bg.get_affinity_by_id(id1, id2);
                }
            }
        }
        None
    }

    /// 根据归一化拼音键检索候选词
    pub fn lookup(&self, key: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();

        // 1. 若挂载了零拷贝 DAT 一体化紧凑词库，直接状态跳转检索
        if let Some(compact) = &self.compact_mmap {
            if let Some(matches) = compact.lookup(key) {
                candidates.reserve(matches.len());
                for (text, score) in matches {
                    let char_len = text.chars().count() as u32;
                    let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                    let final_score = score.saturating_add(boost);
                    candidates.push(AlgoCandidate::new(text, key, final_score));
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

    /// 获取指定拼音键的原始词库条目
    pub fn get_raw_entries(&self, key: &str) -> Option<Vec<(String, u32)>> {
        if let Some(compact) = &self.compact_mmap {
            if let Some(entries) = compact.lookup(key) {
                return Some(entries);
            }
        }
        self.entries.get(key).cloned()
    }

    /// 按需获取指定拼音键的前 `k` 条高频原始词条（用于 Viterbi 束搜索热路径，消除同音长尾无用分配）
    pub fn get_top_raw_entries(&self, key: &str, k: usize) -> Option<Vec<(String, u32)>> {
        if let Some(compact) = &self.compact_mmap {
            if let Some(entries) = compact.lookup_top_k(key, k) {
                return Some(entries);
            }
        }
        self.entries.get(key).map(|list| {
            let mut top = list.clone();
            if top.len() > 1 {
                top.sort_by_key(|b| std::cmp::Reverse(b.1));
            }
            top.truncate(k);
            top
        })
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
