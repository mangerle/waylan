use super::AuxiliaryDict;
use crate::types::{AlgoCandidate, CandidateTier};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

static DEFAULT_ENGLISH_MANAGER: OnceLock<Arc<EnglishSuggestionManager>> = OnceLock::new();
static ACTIVE_ENGLISH_MANAGER: OnceLock<RwLock<Arc<EnglishSuggestionManager>>> = OnceLock::new();
static DEFAULT_SYMBOL_MANAGER: OnceLock<Arc<SymbolSuggestionManager>> = OnceLock::new();

/// 常用高频英文单词联想管理器
#[derive(Debug, Clone)]
pub struct EnglishSuggestionManager {
    /// 单词与词频列表：(小写检索键, 原始词形, 词频)，按小写字母序排序便于零分配二分前缀查找
    words: Vec<(String, String, u32)>,
    /// 精确匹配哈希索引 (小写单词 -> (原词, 词频))
    exact_index: HashMap<String, (String, u32)>,
}

impl Default for EnglishSuggestionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl EnglishSuggestionManager {
    /// 获取全局单例缓存的默认英文单词联想管理器
    pub fn default_global() -> Arc<Self> {
        DEFAULT_ENGLISH_MANAGER
            .get_or_init(|| Arc::new(Self::new()))
            .clone()
    }

    /// 获取当前激活的共享英文词库管理器（未设置外部词库时回退到内置默认词库）
    pub fn active_or_default() -> Arc<Self> {
        let lock = ACTIVE_ENGLISH_MANAGER.get_or_init(|| RwLock::new(Self::default_global()));
        lock.read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| Self::default_global())
    }

    /// 更新当前激活的共享英文词库管理器，供词图级中英混输直接复用外部完整词库
    pub fn set_active_global(manager: Arc<Self>) {
        let lock = ACTIVE_ENGLISH_MANAGER.get_or_init(|| RwLock::new(Self::default_global()));
        if let Ok(mut guard) = lock.write() {
            *guard = manager;
        }
    }

    /// 零堆分配检索文本前缀中精确命中的英文单词（仅扫描连续 ASCII 字母段）
    pub fn match_exact_prefixes(
        &self,
        text: &str,
        min_len: usize,
        max_len: usize,
    ) -> Vec<(&str, u32, usize)> {
        let bytes = text.as_bytes();
        let cap = bytes.len().min(max_len).min(32);
        let mut lower_buf = [0u8; 32];
        let mut ascii_len = 0usize;
        while ascii_len < cap && bytes[ascii_len].is_ascii_alphabetic() {
            lower_buf[ascii_len] = bytes[ascii_len].to_ascii_lowercase();
            ascii_len += 1;
        }
        if ascii_len < min_len {
            return Vec::new();
        }
        let mut matches = Vec::new();
        for len in min_len..=ascii_len {
            if let Ok(key) = std::str::from_utf8(&lower_buf[..len]) {
                if let Some((orig, score)) = self.exact_index.get(key) {
                    matches.push((orig.as_str(), *score, len));
                }
            }
        }
        matches
    }

    /// 创建带默认保底词汇的管理器
    pub fn new() -> Self {
        Self::from_words(AuxiliaryDict::default_english_words())
    }

    /// 从单词列表加载
    pub fn from_words(words: Vec<(String, u32)>) -> Self {
        let mut exact_index = HashMap::with_capacity(words.len());
        let mut sorted_words = Vec::with_capacity(words.len());
        for (w, score) in words {
            let lower = w.to_lowercase();
            exact_index.insert(lower.clone(), (w.clone(), score));
            sorted_words.push((lower, w, score));
        }
        sorted_words.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            words: sorted_words,
            exact_index,
        }
    }

    /// 获取当前词目总数
    pub fn len(&self) -> usize {
        self.exact_index.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.exact_index.is_empty()
    }

    /// 匹配输入前缀对应的英文单词
    pub fn lookup(&self, input: &str) -> Vec<AlgoCandidate> {
        let input_lower = input.to_lowercase();
        let mut results = Vec::new();

        // 1. 完全精确匹配 (O(1) 哈希命中)
        if let Some((orig_word, score)) = self.exact_index.get(&input_lower) {
            results.push(
                AlgoCandidate::new(orig_word, input, 4_500_000_u32.saturating_add(*score))
                    .with_comment("英文单词")
                    .with_tier(CandidateTier::Acronym),
            );
            return results;
        }

        // 2. 前缀匹配（输入至少 3 个字母时提供补全联想，利用预存小写键零分配二分查找定位）
        if input_lower.len() >= 3 {
            let idx = match self
                .words
                .binary_search_by(|(w_lower, _, _)| w_lower.as_str().cmp(input_lower.as_str()))
            {
                Ok(i) => i,
                Err(i) => i,
            };

            let mut prefix_cands = Vec::new();
            for (w_lower, orig_word, score) in &self.words[idx..] {
                if w_lower.starts_with(&input_lower) {
                    if w_lower != &input_lower {
                        prefix_cands.push(
                            AlgoCandidate::new(
                                orig_word,
                                input,
                                3_000_000_u32.saturating_add(*score / 2),
                            )
                            .with_comment("英文联想")
                            .with_tier(CandidateTier::PrefixPartial),
                        );
                    }
                } else {
                    break;
                }
            }
            prefix_cands.sort_by_key(|c| std::cmp::Reverse(c.score));
            prefix_cands.truncate(3);
            results.extend(prefix_cands);
        }

        results
    }
}

/// 常用表情与特殊符号联想管理器（严格使用 Unicode 码点，代码内无 Emoji 字符）
#[derive(Debug, Clone)]
pub struct SymbolSuggestionManager {
    /// 拼音键到 (符号文本, 描述) 的哈希索引映射
    index: HashMap<String, Vec<(String, String)>>,
    /// 符号条目总数
    total_len: usize,
}

impl Default for SymbolSuggestionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolSuggestionManager {
    /// 获取全局单例缓存的默认符号表情联想管理器
    pub fn default_global() -> Arc<Self> {
        DEFAULT_SYMBOL_MANAGER
            .get_or_init(|| Arc::new(Self::new()))
            .clone()
    }

    /// 创建带默认保底符号表的管理器
    pub fn new() -> Self {
        Self::from_symbols(AuxiliaryDict::default_symbols())
    }

    /// 从符号列表加载构建哈希倒排索引
    pub fn from_symbols(symbols: Vec<(String, String, String)>) -> Self {
        let total_len = symbols.len();
        let mut index: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (py, sym, desc) in symbols {
            index
                .entry(py.to_lowercase())
                .or_default()
                .push((sym, desc));
        }
        Self { index, total_len }
    }

    /// 获取当前符号总数
    pub fn len(&self) -> usize {
        self.total_len
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.total_len == 0
    }

    /// 检索符号表情候选 (O(1) 哈希查询)
    pub fn lookup(&self, pinyin: &str) -> Vec<AlgoCandidate> {
        let pinyin_clean = pinyin.replace('\'', "").to_lowercase();
        let mut results = Vec::new();
        if let Some(list) = self.index.get(&pinyin_clean) {
            for (sym, desc) in list {
                results.push(
                    AlgoCandidate::new(sym, pinyin, 4_800_000)
                        .with_comment(desc)
                        .with_tier(CandidateTier::Acronym),
                );
            }
        }
        results
    }
}

/// 按照多级搜索策略加载并构建辅助与混输词库管理器
///
/// 若磁盘存在用户定制或预编译辅助词库，则从文件反序列化构建；若不存在则零拷贝复用内置全局默认实例。
pub fn load_auxiliary_managers() -> (Arc<EnglishSuggestionManager>, Arc<SymbolSuggestionManager>) {
    let search_paths = waylan_core::AppConfig::aux_dict_search_paths();

    for path in search_paths {
        if path.exists() {
            if let Ok(dict) = AuxiliaryDict::load_from_file(&path) {
                let en = Arc::new(EnglishSuggestionManager::from_words(dict.english_words));
                let sym = Arc::new(SymbolSuggestionManager::from_symbols(dict.symbols));
                return (en, sym);
            }
        }
    }

    (
        EnglishSuggestionManager::default_global(),
        SymbolSuggestionManager::default_global(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_english_prefix_lookup_top3_score_ordering() {
        let words = vec![
            ("appal".to_string(), 10),
            ("apparel".to_string(), 20),
            ("apple".to_string(), 1000),
            ("application".to_string(), 900),
            ("apply".to_string(), 500),
        ];
        let manager = EnglishSuggestionManager::from_words(words);
        let res = manager.lookup("app");
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].text, "apple");
        assert_eq!(res[1].text, "application");
        assert_eq!(res[2].text, "apply");
    }

    #[test]
    fn test_symbol_lookup_case_insensitive() {
        let symbols = vec![("wx".to_string(), "^_^".to_string(), "微笑".to_string())];
        let manager = SymbolSuggestionManager::from_symbols(symbols);
        let res_lower = manager.lookup("wx");
        assert_eq!(res_lower.len(), 1);
        assert_eq!(res_lower[0].text, "^_^");

        let res_upper = manager.lookup("WX");
        assert_eq!(res_upper.len(), 1);
        assert_eq!(res_upper[0].text, "^_^");
    }

    #[test]
    fn test_default_global_singleton() {
        let en1 = EnglishSuggestionManager::default_global();
        let en2 = EnglishSuggestionManager::default_global();
        assert!(
            Arc::ptr_eq(&en1, &en2),
            "多次获取英文单例应共享同一 Arc 指针"
        );
        assert!(!en1.is_empty(), "默认英文词库不应为空");

        let sym1 = SymbolSuggestionManager::default_global();
        let sym2 = SymbolSuggestionManager::default_global();
        assert!(
            Arc::ptr_eq(&sym1, &sym2),
            "多次获取符号单例应共享同一 Arc 指针"
        );
        assert!(!sym1.is_empty(), "默认符号词库不应为空");
    }
}
