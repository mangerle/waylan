use crate::bigram::CompactMmapBigram;
use crate::vocab::Vocabulary;
use crate::error::AlgoError;
use crate::prism::STANDARD_INITIALS;
use crate::syllables::STANDARD_SYLLABLES;
use crate::types::{AlgoCandidate, CandidateTier};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

/// 紧凑词典魔数标识（16 字节）
pub const COMPACT_DICT_MAGIC: &[u8; 16] = b"WAYLAN_COMPACT01";

/// 零拷贝紧凑只读词典 (Compact Mmap Lexicon)
///
/// # 设计原理
/// - **实现初衷**：替换传统 bincode 全量反序列化带来的数十万次堆对象分配与百毫秒级冷启动延迟；
/// - **核心优势**：通过 mmap 映射只读二进制文件，启动耗时仅需数十微秒，内存按需由 OS 分页置换；
/// - **检索算法**：键索引表在离线编译期已按字典序完成排序，在线检索通过二分查找在 O(log N) 纳秒级完成。
pub struct CompactMmapLexicon {
    mmap: Mmap,
    pub num_entries: usize,
    pub num_acronyms: usize,
    pub num_initials: usize,
    entries_offset: usize,
    acronyms_offset: usize,
    initials_offset: usize,
    values_offset: usize,
    strings_offset: usize,
}

impl std::fmt::Debug for CompactMmapLexicon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactMmapLexicon")
            .field("num_entries", &self.num_entries)
            .field("num_acronyms", &self.num_acronyms)
            .field("num_initials", &self.num_initials)
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
        if mmap.len() < 32 || &mmap[0..16] != COMPACT_DICT_MAGIC {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom("无效的紧凑词典魔数标识".to_string()).into(),
            });
        }
        let num_entries = u32::from_le_bytes(mmap[16..20].try_into().unwrap_or([0; 4])) as usize;
        let num_acronyms = u32::from_le_bytes(mmap[20..24].try_into().unwrap_or([0; 4])) as usize;
        let num_initials = u32::from_le_bytes(mmap[24..28].try_into().unwrap_or([0; 4])) as usize;

        let entries_offset = 32;
        let acronyms_offset = entries_offset + num_entries * 16;
        let initials_offset = acronyms_offset + num_acronyms * 16;
        let values_meta_pos = initials_offset + num_initials * 16;

        if mmap.len() < values_meta_pos + 8 {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom("紧凑词典头部截断".to_string()).into(),
            });
        }

        let num_values = u32::from_le_bytes(mmap[values_meta_pos..values_meta_pos + 4].try_into().unwrap_or([0; 4])) as usize;
        let strings_len = u32::from_le_bytes(mmap[values_meta_pos + 4..values_meta_pos + 8].try_into().unwrap_or([0; 4])) as usize;

        let values_offset = values_meta_pos + 8;
        let strings_offset = values_offset + num_values * 12;

        if mmap.len() < strings_offset + strings_len {
            return Err(AlgoError::DeserializeFailed {
                path: path.to_path_buf(),
                source: bincode::ErrorKind::Custom("紧凑词典数据区截断".to_string()).into(),
            });
        }

        Ok(Self {
            mmap,
            num_entries,
            num_acronyms,
            num_initials,
            entries_offset,
            acronyms_offset,
            initials_offset,
            values_offset,
            strings_offset,
        })
    }

    /// 获取指定索引段的键字符串
    fn get_key(&self, base_offset: usize, idx: usize) -> &str {
        let off = base_offset + idx * 16;
        let k_off = u32::from_le_bytes(self.mmap[off..off + 4].try_into().unwrap_or([0; 4])) as usize;
        let k_len = u16::from_le_bytes(self.mmap[off + 4..off + 6].try_into().unwrap_or([0; 2])) as usize;
        let s_start = self.strings_offset + k_off;
        std::str::from_utf8(&self.mmap[s_start..s_start + k_len]).unwrap_or("")
    }

    /// 获取指定索引段条目的候选值列表
    fn get_values(&self, base_offset: usize, idx: usize) -> Vec<(String, u32)> {
        let off = base_offset + idx * 16;
        let v_off = u32::from_le_bytes(self.mmap[off + 8..off + 12].try_into().unwrap_or([0; 4])) as usize;
        let v_count = u32::from_le_bytes(self.mmap[off + 12..off + 16].try_into().unwrap_or([0; 4])) as usize;

        let mut res = Vec::with_capacity(v_count);
        for i in 0..v_count {
            let item_off = self.values_offset + (v_off + i) * 12;
            let t_off = u32::from_le_bytes(self.mmap[item_off..item_off + 4].try_into().unwrap_or([0; 4])) as usize;
            let t_len = u16::from_le_bytes(self.mmap[item_off + 4..item_off + 6].try_into().unwrap_or([0; 2])) as usize;
            let score = u32::from_le_bytes(self.mmap[item_off + 8..item_off + 12].try_into().unwrap_or([0; 4]));
            let s_start = self.strings_offset + t_off;
            let text = std::str::from_utf8(&self.mmap[s_start..s_start + t_len]).unwrap_or("");
            res.push((text.to_string(), score));
        }
        res
    }

    /// 全拼二分检索
    pub fn lookup(&self, key: &str) -> Option<Vec<(String, u32)>> {
        let mut left = 0;
        let mut right = self.num_entries;
        while left < right {
            let mid = left + (right - left) / 2;
            let mid_key = self.get_key(self.entries_offset, mid);
            match mid_key.cmp(key) {
                std::cmp::Ordering::Equal => return Some(self.get_values(self.entries_offset, mid)),
                std::cmp::Ordering::Less => left = mid + 1,
                std::cmp::Ordering::Greater => right = mid,
            }
        }
        None
    }

    /// 简拼二分检索
    pub fn lookup_acronym(&self, key: &str) -> Option<Vec<(String, u32)>> {
        let mut left = 0;
        let mut right = self.num_acronyms;
        while left < right {
            let mid = left + (right - left) / 2;
            let mid_key = self.get_key(self.acronyms_offset, mid);
            match mid_key.cmp(key) {
                std::cmp::Ordering::Equal => return Some(self.get_values(self.acronyms_offset, mid)),
                std::cmp::Ordering::Less => left = mid + 1,
                std::cmp::Ordering::Greater => right = mid,
            }
        }
        None
    }

    /// 声母二分检索
    pub fn lookup_initial(&self, initial: &str) -> Option<Vec<(String, u32)>> {
        let mut left = 0;
        let mut right = self.num_initials;
        while left < right {
            let mid = left + (right - left) / 2;
            let mid_key = self.get_key(self.initials_offset, mid);
            match mid_key.cmp(initial) {
                std::cmp::Ordering::Equal => return Some(self.get_values(self.initials_offset, mid)),
                std::cmp::Ordering::Less => left = mid + 1,
                std::cmp::Ordering::Greater => right = mid,
            }
        }
        None
    }

    /// 提取全量 entries 用于向后兼容完整填充内存表
    pub fn dump_to_maps(
        &self,
    ) -> (
        HashMap<String, Vec<(String, u32)>>,
        HashMap<String, Vec<(String, u32)>>,
        HashMap<String, Vec<(String, u32)>>,
    ) {
        let mut entries = HashMap::with_capacity(self.num_entries);
        for i in 0..self.num_entries {
            let k = self.get_key(self.entries_offset, i).to_string();
            let v = self.get_values(self.entries_offset, i);
            entries.insert(k, v);
        }

        let mut acronyms = HashMap::with_capacity(self.num_acronyms);
        for i in 0..self.num_acronyms {
            let k = self.get_key(self.acronyms_offset, i).to_string();
            let v = self.get_values(self.acronyms_offset, i);
            acronyms.insert(k, v);
        }

        let mut initials = HashMap::with_capacity(self.num_initials);
        for i in 0..self.num_initials {
            let k = self.get_key(self.initials_offset, i).to_string();
            let v = self.get_values(self.initials_offset, i);
            initials.insert(k, v);
        }

        (entries, acronyms, initials)
    }
}

/// 紧凑词典索引表 (Table / Lexicon)
///
/// # 设计原理
/// - **实现初衷**：负责将音节序列（如 "ni'hao"）映射到候选字词列表及基准静态词频，
///   向下兼容项目现有的二进制紧凑格式（`base.dict.bin`），并原生集成零拷贝 mmap。
/// - **核心优势**：在内存中以只读哈希表与连续向量存储，检索为 O(1) 微秒级操作；支持零拷贝二进制加载。
#[derive(Debug)]
pub struct LexiconTable {
    /// 倒排拼音索引表：键为拼音序列（如 "ni'hao" 或 "xian"），值为候选文本与基础静态分
    entries: HashMap<String, Vec<(String, u32)>>,
    /// 简拼缩写索引表：键为首字母缩写（如 "bj" 或 "zg"），值为候选文本与基础静态分
    acronym_entries: HashMap<String, Vec<(String, u32)>>,
    /// 标准声母高频单字索引表（如 "s", "d", "g"），O(1) 零分配切片返回
    initial_entries: HashMap<String, Vec<(String, u32)>>,
    /// 零拷贝紧凑只读词典（通过 mmap 映射）
    compact_mmap: Option<CompactMmapLexicon>,
    /// 零拷贝紧凑只读转移概率矩阵（CSR 微信键盘同级高密度架构）
    pub compact_bigram: Option<CompactMmapBigram>,
    /// 全局词表（用于 CSR 架构将词文本映射为 Word ID）
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

    /// 插入一条字词条目
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

    /// 从预编译二进制词库文件中加载
    ///
    /// # 架构特性
    /// 从预编译二进制紧凑词库文件中加载（零拷贝内存映射）
    ///
    /// # 架构特性
    /// 强制校验零拷贝 mmap 紧凑词库格式（魔数 `WAYLAN_COMPACT01`），实现数十微秒闪电加载。
    /// 若文件损坏或魔数不匹配，立即返回错误，严禁隐式性能退化。
    ///
    /// # Errors
    /// 当文件路径不存在或反序列化失败时返回 [`AlgoError::DictionaryIo`] 或 [`AlgoError::DeserializeFailed`]。
    pub fn load_binary(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        let metadata = file.metadata().map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        if metadata.len() < 32 {
            return Err(AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: bincode::ErrorKind::Custom("词典文件体积过小或已损坏".to_string()).into(),
            });
        }

        let mmap = unsafe {
            Mmap::map(&file).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?
        };

        let compact = CompactMmapLexicon::from_mmap(mmap, path_ref)?;

        // 自动探测并挂载全局词表与 CSR 高密度转移概率矩阵
        let vocab_path = path_ref.with_file_name("vocab.bin");
        let vocab = if vocab_path.exists() {
            Vocabulary::load_binary(&vocab_path).ok()
        } else {
            None
        };

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
            vocab,
        })
    }

    /// 手动挂载外部零拷贝 Bigram 转移概率矩阵与配套全局词表
    pub fn load_bigram(
        &mut self,
        bigram_path: impl AsRef<Path>,
        vocab_path: impl AsRef<Path>,
    ) -> Result<(), AlgoError> {
        let bg = CompactMmapBigram::open(bigram_path)?;
        let vocab = Vocabulary::load_binary(vocab_path)?;
        self.compact_bigram = Some(bg);
        self.vocab = Some(vocab);
        Ok(())
    }

    /// 查询二元词对统计转移亲和度加权得分
    ///
    /// 通过 `vocab` 将文本换算为整数 Word ID，并在高密度 CSR 矩阵内执行纳秒级局部二分查找；
    /// 若未收录则返回 `None`（交由上层平滑回退至规则奖励）。
    #[inline]
    pub fn get_bigram_affinity(&self, prev: &str, curr: &str) -> Option<i64> {
        if let (Some(bg), Some(vocab)) = (&self.compact_bigram, &self.vocab) {
            if let (Some(id1), Some(id2)) = (vocab.get_id(prev), vocab.get_id(curr)) {
                return bg.get_affinity_by_id(id1, id2);
            }
        }
        None
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

    /// 根据归一化拼音键检索候选词（支持一级核心库优先，二级扩展库长尾补充）
    pub fn lookup(&self, key: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();

        // 1. 若挂载了零拷贝紧凑词库，直接二分查找
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

        // 2. 检索内存动态词表（兼容 sample_data 与动态追加条目）
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

    /// 获取指定拼音键的原始词库条目（包含原始字词文本与未经加权的基准静态词频）
    pub fn get_raw_entries(&self, key: &str) -> Option<Vec<(String, u32)>> {
        if let Some(compact) = &self.compact_mmap {
            if let Some(entries) = compact.lookup(key) {
                return Some(entries);
            }
        }
        self.entries.get(key).cloned()
    }

    /// 检索指定单字母声母（如 "g", "d", "s"）对应的高频单字原始词库条目列表
    pub fn get_initial_raw_entries(&self, initial: &str) -> Option<Vec<(String, u32)>> {
        if let Some(compact) = &self.compact_mmap {
            if let Some(entries) = compact.lookup_initial(initial) {
                return Some(entries);
            }
        }
        self.initial_entries.get(initial).cloned()
    }

    /// 检索带末尾残存拼音前缀的补全候选词列表
    ///
    /// 典型场景：用户输入 "shux"，切分为音节 ["shu"] 与未完成声母 "x"；
    /// 本方法自动枚举以 'x' 开头的合法音节（如 xi, xue, xing ...），组合成 "shu'xi", "shu'xue" 等在词库中检索高频词。
    pub fn lookup_prefix_extension(
        &self,
        prefix_syllables: &[String],
        remaining_initial: &str,
    ) -> Vec<AlgoCandidate> {
        if remaining_initial.is_empty() {
            return Vec::new();
        }

        let mut candidates = Vec::new();

        if prefix_syllables.is_empty() {
            // 单声母前缀检索：枚举以 remaining_initial 开头的高频单字
            for &syl in STANDARD_SYLLABLES {
                if syl.starts_with(remaining_initial) {
                    if let Some(matches) = self.get_raw_entries(syl) {
                        for (text, score) in matches {
                            if text.chars().count() == 1 {
                                candidates.push(
                                    AlgoCandidate::new(text, syl, score)
                                        .with_tier(CandidateTier::ExactWord),
                                );
                            }
                        }
                    }
                }
            }
        } else {
            // 多音节+末尾声母前缀检索：组合全拼并召回成熟词条
            let prefix_key = prefix_syllables.join("'");
            for &syl in STANDARD_SYLLABLES {
                if syl.starts_with(remaining_initial) {
                    let full_key = format!("{}'{}", prefix_key, syl);
                    if let Some(matches) = self.get_raw_entries(&full_key) {
                        for (text, score) in matches {
                            let char_len = text.chars().count() as u32;
                            let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                            let final_score = score.saturating_add(boost);
                            candidates.push(
                                AlgoCandidate::new(text, full_key.clone(), final_score)
                                    .with_tier(CandidateTier::ExactWord),
                            );
                        }
                    }
                }
            }
        }

        // 按得分降序排序，限制最多保留 20 条候选，避免过多长尾生僻条目
        candidates.sort_by_key(|c| std::cmp::Reverse(c.score));
        candidates.truncate(20);
        candidates
    }

    /// 检索首字母缩写简拼候选词（如 "bj" 匹配 "北京"，"yyds" 匹配 "永远的神"）
    pub fn lookup_acronym(&self, key: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();

        if let Some(compact) = &self.compact_mmap {
            if let Some(matches) = compact.lookup_acronym(key) {
                for (text, score) in matches {
                    candidates.push(
                        AlgoCandidate::new(text, key, score)
                            .with_tier(CandidateTier::Acronym)
                            .with_comment("[简拼]"),
                    );
                }
            }
        }

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
        let compact_len = self.compact_mmap.as_ref().map(|c| c.num_entries).unwrap_or(0);
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
