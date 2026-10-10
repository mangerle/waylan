use crate::custom_phrase::AuxiliaryDict;
use crate::types::{AlgoCandidate, CandidateTier};
use std::collections::HashMap;

/// 常用高频英文单词联想管理器
#[derive(Debug, Clone)]
pub struct EnglishSuggestionManager {
    /// 单词与词频列表（按小写字母序排序，便于二分前缀查找）
    words: Vec<(String, u32)>,
    /// 精确匹配哈希索引 (小写单词 -> (原词, 词频))
    exact_index: HashMap<String, (String, u32)>,
}

impl Default for EnglishSuggestionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl EnglishSuggestionManager {
    /// 创建带默认保底词汇的管理器
    pub fn new() -> Self {
        Self::from_words(AuxiliaryDict::default_english_words())
    }

    /// 从单词列表加载
    pub fn from_words(words: Vec<(String, u32)>) -> Self {
        let mut exact_index = HashMap::with_capacity(words.len());
        for (w, score) in &words {
            exact_index.insert(w.to_lowercase(), (w.clone(), *score));
        }
        let mut sorted_words = words;
        sorted_words.sort_by_key(|a| a.0.to_lowercase());
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

        // 2. 前缀匹配（输入至少 3 个字母时提供补全联想，利用二分查找定位）
        if input_lower.len() >= 3 {
            let idx = match self
                .words
                .binary_search_by(|(w, _)| w.to_lowercase().cmp(&input_lower))
            {
                Ok(i) => i,
                Err(i) => i,
            };

            for (w, score) in &self.words[idx..] {
                let w_lower = w.to_lowercase();
                if w_lower.starts_with(&input_lower) {
                    if w_lower != input_lower {
                        results.push(
                            AlgoCandidate::new(w, input, 3_000_000_u32.saturating_add(*score / 2))
                                .with_comment("英文联想")
                                .with_tier(CandidateTier::PrefixPartial),
                        );
                        if results.len() >= 3 {
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
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
    /// 创建带默认保底符号表的管理器
    pub fn new() -> Self {
        Self::from_symbols(AuxiliaryDict::default_symbols())
    }

    /// 从符号列表加载构建哈希倒排索引
    pub fn from_symbols(symbols: Vec<(String, String, String)>) -> Self {
        let total_len = symbols.len();
        let mut index: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (py, sym, desc) in symbols {
            index.entry(py).or_default().push((sym, desc));
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
        let pinyin_clean = pinyin.replace('\'', "");
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
