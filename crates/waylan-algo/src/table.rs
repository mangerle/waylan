use crate::bigram::CompactMmapBigram;
use crate::dat::DatReader;
use crate::error::AlgoError;
use crate::prism::STANDARD_INITIALS;
use crate::types::AlgoCandidate;
use crate::vocab::Vocabulary;
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

/// 紧凑词典魔数标识（16 字节）
pub const COMPACT_DICT_MAGIC: &[u8; 16] = b"WAYLAN_COMPACT02";

/// 零拷贝紧凑只读词典 (Compact Mmap Lexicon - DAT 一体化版)
///
/// # 设计原理
/// - **实现初衷**：将拼音双数组 Trie (DAT)、高密度候选池与单源词表融合为一个紧凑二进制资产；
/// - **核心优势**：
///   1. 彻底消灭明文拼音存储，检索严格为 $O(L)$ 状态跳转；
///   2. 词条文本单源只读存储，同时承担反查与正查，消灭独立词表文件；
///   3. 零堆分配，零反序列化，由操作系统虚拟内存按需分页置换。
pub struct CompactMmapLexicon {
    mmap: Mmap,
    pub num_keys: usize,
    pub num_candidates: usize,
    pub vocab_size: usize,
    dat_node_count: usize,
    dat_tail_len: usize,
    dat_base_offset: usize,
    dat_check_offset: usize,
    dat_tail_offset: usize,
    key_metas_offset: usize,
    candidates_offset: usize,
    vocab_offsets_offset: usize,
    sorted_vocab_ids_offset: usize,
    vocab_strings_offset: usize,
}

impl std::fmt::Debug for CompactMmapLexicon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactMmapLexicon")
            .field("num_keys", &self.num_keys)
            .field("num_candidates", &self.num_candidates)
            .field("vocab_size", &self.vocab_size)
            .finish()
    }
}

impl CompactMmapLexicon {
    /// 打开紧凑二进制词库文件并建立零拷贝内存映射
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?
        };
        Self::from_mmap(mmap, path_ref)
    }

    /// 从已就绪的内存切片解析紧凑只读词典结构
    pub fn from_mmap(mmap: Mmap, path: &Path) -> Result<Self, AlgoError> {
        if mmap.len() < 64 || &mmap[0..16] != COMPACT_DICT_MAGIC {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom(
                    "无效的紧凑词典魔数标识 (需 WAYLAN_COMPACT02)".to_string(),
                )
                .into(),
            });
        }

        let version = u32::from_le_bytes(mmap[16..20].try_into().unwrap_or([0; 4]));
        if version != 2 {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom(format!("不支持的词库版本号: {}", version))
                    .into(),
            });
        }

        let num_keys = u32::from_le_bytes(mmap[20..24].try_into().unwrap_or([0; 4])) as usize;
        let num_candidates = u32::from_le_bytes(mmap[24..28].try_into().unwrap_or([0; 4])) as usize;
        let vocab_size = u32::from_le_bytes(mmap[28..32].try_into().unwrap_or([0; 4])) as usize;
        let dat_node_count = u32::from_le_bytes(mmap[32..36].try_into().unwrap_or([0; 4])) as usize;
        let dat_tail_len = u32::from_le_bytes(mmap[36..40].try_into().unwrap_or([0; 4])) as usize;

        let key_metas_offset =
            u64::from_le_bytes(mmap[40..48].try_into().unwrap_or([0; 8])) as usize;
        let candidates_offset =
            u64::from_le_bytes(mmap[48..56].try_into().unwrap_or([0; 8])) as usize;
        let vocab_offset = u64::from_le_bytes(mmap[56..64].try_into().unwrap_or([0; 8])) as usize;

        let dat_base_offset = 64usize;
        let dat_check_offset = dat_base_offset.saturating_add(dat_node_count.saturating_mul(4));
        let dat_tail_offset = dat_check_offset.saturating_add(dat_node_count.saturating_mul(4));
        let dat_tail_end = dat_tail_offset.saturating_add(dat_tail_len);
        let key_metas_end = key_metas_offset.saturating_add(num_keys.saturating_mul(8));
        let candidates_end = candidates_offset.saturating_add(num_candidates.saturating_mul(6));

        let vocab_offsets_offset = vocab_offset;
        let sorted_vocab_ids_offset =
            vocab_offsets_offset.saturating_add(vocab_size.saturating_add(1).saturating_mul(4));
        let vocab_strings_offset =
            sorted_vocab_ids_offset.saturating_add(vocab_size.saturating_mul(4));

        let min_required_len = dat_tail_end
            .max(key_metas_end)
            .max(candidates_end)
            .max(vocab_strings_offset);

        if mmap.len() < min_required_len {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom("紧凑词典数据区截断".to_string()).into(),
            });
        }

        Ok(Self {
            mmap,
            num_keys,
            num_candidates,
            vocab_size,
            dat_node_count,
            dat_tail_len,
            dat_base_offset,
            dat_check_offset,
            dat_tail_offset,
            key_metas_offset,
            candidates_offset,
            vocab_offsets_offset,
            sorted_vocab_ids_offset,
            vocab_strings_offset,
        })
    }

    /// 获取只读 DAT 视图
    #[inline]
    fn get_dat_reader(&self) -> DatReader<'_> {
        let base_slice = unsafe {
            std::slice::from_raw_parts(
                self.mmap[self.dat_base_offset..].as_ptr() as *const i32,
                self.dat_node_count,
            )
        };
        let check_slice = unsafe {
            std::slice::from_raw_parts(
                self.mmap[self.dat_check_offset..].as_ptr() as *const u32,
                self.dat_node_count,
            )
        };
        let tail_slice = &self.mmap[self.dat_tail_offset..self.dat_tail_offset + self.dat_tail_len];
        DatReader::new(base_slice, check_slice, tail_slice)
    }

    /// 根据 Word ID 反查对应的词条文本（O(1) 零拷贝切片返回）
    #[inline]
    pub fn get_word(&self, word_id: u32) -> Option<&str> {
        let wid = word_id as usize;
        if wid >= self.vocab_size {
            return None;
        }

        let off1_pos = self
            .vocab_offsets_offset
            .saturating_add(wid.saturating_mul(4));
        let off2_pos = off1_pos.saturating_add(4);
        let start =
            u32::from_le_bytes(self.mmap.get(off1_pos..off1_pos + 4)?.try_into().ok()?) as usize;
        let end =
            u32::from_le_bytes(self.mmap.get(off2_pos..off2_pos + 4)?.try_into().ok()?) as usize;

        let s_start = self.vocab_strings_offset.saturating_add(start);
        let s_end = self.vocab_strings_offset.saturating_add(end);
        let slice = self.mmap.get(s_start..s_end)?;

        std::str::from_utf8(slice).ok()
    }

    /// 根据词条文本快速二分反查对应的 Word ID（纳秒级）
    pub fn get_id(&self, word: &str) -> Option<u32> {
        if self.vocab_size == 0 {
            return None;
        }

        let mut left = 0usize;
        let mut right = self.vocab_size;

        while left < right {
            let mid = left + (right - left) / 2;
            let id_pos = self
                .sorted_vocab_ids_offset
                .saturating_add(mid.saturating_mul(4));
            let cand_id = u32::from_le_bytes(self.mmap.get(id_pos..id_pos + 4)?.try_into().ok()?);
            let cand_word = self.get_word(cand_id).unwrap_or("");

            match cand_word.cmp(word) {
                std::cmp::Ordering::Equal => return Some(cand_id),
                std::cmp::Ordering::Less => left = mid + 1,
                std::cmp::Ordering::Greater => right = mid,
            }
        }

        None
    }

    /// 获取指定 key_idx 的候选列表
    fn get_candidates_by_key_idx(
        &self,
        key_idx: usize,
        expected_type: Option<u8>,
    ) -> Option<Vec<(String, u32)>> {
        if key_idx >= self.num_keys {
            return None;
        }

        let meta_pos = self
            .key_metas_offset
            .saturating_add(key_idx.saturating_mul(8));
        let meta_slice = self.mmap.get(meta_pos..meta_pos + 8)?;
        let cand_start = u32::from_le_bytes(meta_slice[0..4].try_into().ok()?) as usize;
        let cand_count = u16::from_le_bytes(meta_slice[4..6].try_into().ok()?) as usize;
        let key_type = meta_slice[6];

        if let Some(exp_type) = expected_type {
            if key_type != exp_type {
                return None;
            }
        }

        let mut res = Vec::with_capacity(cand_count);
        for i in 0..cand_count {
            let item_pos = self
                .candidates_offset
                .saturating_add(cand_start.saturating_add(i).saturating_mul(6));
            let item_slice = self.mmap.get(item_pos..item_pos + 6)?;
            let word_id = u32::from_le_bytes(item_slice[0..4].try_into().ok()?);
            let score_u16 = u16::from_le_bytes(item_slice[4..6].try_into().ok()?);
            // 还原静态分，并对达到 u16 上限的候选附加原始顺位梯度以保留同音高频词相对次序
            let base_score = decode_compact_score(score_u16);
            let rank_bonus = cand_count.saturating_sub(i).min(64) as u32;
            let score = base_score.saturating_add(rank_bonus);

            if let Some(text) = self.get_word(word_id) {
                res.push((text.to_string(), score));
            }
        }

        Some(res)
    }

    /// 全拼检索（通过 DAT 精确匹配）
    pub fn lookup(&self, key: &str) -> Option<Vec<(String, u32)>> {
        let reader = self.get_dat_reader();
        let key_idx = reader.exact_match(key)? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(0))
    }

    /// 简拼缩写检索
    pub fn lookup_acronym(&self, key: &str) -> Option<Vec<(String, u32)>> {
        let reader = self.get_dat_reader();
        let prefixed = format!("'{}", key);
        let key_idx = reader
            .exact_match(&prefixed)
            .or_else(|| reader.exact_match(key))? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(1))
    }

    /// 声母高频单字检索
    pub fn lookup_initial(&self, initial: &str) -> Option<Vec<(String, u32)>> {
        let reader = self.get_dat_reader();
        let prefixed = format!("''{}", initial);
        let key_idx = reader
            .exact_match(&prefixed)
            .or_else(|| reader.exact_match(initial))? as usize;
        self.get_candidates_by_key_idx(key_idx, Some(2))
    }
}

/// 将 16 位紧凑存储词频还原为 32 位静态分（兼容旧版 65535 截断与新版分段对数压缩）
#[inline]
fn decode_compact_score(score_u16: u16) -> u32 {
    score_u16 as u32
}

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
