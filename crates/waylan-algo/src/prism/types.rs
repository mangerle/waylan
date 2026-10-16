use super::fuzzy::PrismFuzzy;
use crate::syllables::is_valid_syllable;
use crate::types::FuzzyConfig;

/// 完整音节切分路径元数据契约
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPath {
    /// 完整音节切分列表（如 ["xi", "an"] 或 ["xian"]）
    pub syllables: Vec<String>,
    /// 末尾未完成键入的剩余前缀（如 "h"）
    pub remaining: String,
    /// 路径代价值（段数越少、匹配越长代价越低，优先排序）
    pub cost: u32,
    /// 是否由容错纠错虚边衍生生成
    pub is_typo: bool,
}

impl SegmentPath {
    /// 转换为带单引号的展示字符串（如 "xi'an"）
    pub fn to_key_string(&self) -> String {
        let mut parts = self.syllables.clone();
        if !self.remaining.is_empty() {
            parts.push(self.remaining.clone());
        }
        parts.join("'")
    }

    /// 判定当前非容错路径是否属于因拼写误触（邻键误按或字母倒置）而产生的断流碎切降级路径
    ///
    /// # 设计原理
    /// - **实现初衷**：区分合法的“全拼/纯简拼/全拼+尾部简拼”（如 `wo'men'de'g'j`）与因拼写笔误被迫拆出
    ///   孤立声母或脱落鼻音韵尾的畸形路径（如 `yingxaing` 碎切为 `ying'x'ai'n'g`、`nihso` 碎切为 `ni'h's'o`）。
    /// - **核心优势**：阻断 Viterbi 整句解码器与跨词前缀合成器在碎切假路径上捏造出“应先爱能够”等生造词，
    ///   并驱动引擎将真正的拼写纠错成词（如“影响”、“影像”）直接置顶。
    pub fn is_broken_typo_fallback(&self, fuzzy: &FuzzyConfig) -> bool {
        if self.is_typo {
            return false;
        }
        let total_units = self.syllables.len() + usize::from(!self.remaining.is_empty());
        if total_units <= 1 {
            return false;
        }

        let is_full_syl = |s: &str| -> bool {
            (is_valid_syllable(s) || PrismFuzzy::is_fuzzy_valid(s, fuzzy))
                && (s.len() >= 2 || matches!(s, "a" | "o" | "e"))
        };

        // 1. 夹在多音节中间的孤立单字母元音借调碎切（如 shi'o'hu, zhu'a'g'n）
        if self.syllables.len() >= 3
            && self.syllables[1..]
                .iter()
                .any(|s| matches!(s.as_str(), "a" | "o" | "e"))
        {
            return true;
        }

        // 2. 孤立辅音声母或单鼻音之后再次出现完整音节或独立鼻音（非纯尾部声母链，如 ying'x'ai'n'g, ni'h's'o）
        let mut seen_non_full = false;
        for syl in &self.syllables {
            if !is_full_syl(syl) {
                seen_non_full = true;
            } else if seen_non_full {
                return true;
            }
        }

        // 3. 后鼻音韵尾 ng 因前驱元音错位而脱落拆散（如 xaing -> x'ai'n'g, cehng -> ce'h'n'g）
        if self.syllables.iter().any(|s| s == "ng") {
            return true;
        }
        for window in self.syllables.windows(2) {
            if (window[0] == "n" && window[1] == "g") || (window[0] == "g" && window[1] == "n") {
                return true;
            }
        }
        if self.syllables.last().is_some_and(|last| last == "n") && self.remaining == "g" {
            return true;
        }

        false
    }

    /// 按照当前路径的音节字节跨度对原始按键序列进行单引号切分（用于容错态保真预编辑展示）
    pub fn format_raw_with_spans(&self, raw_input: &str) -> Option<String> {
        let total_len: usize =
            self.syllables.iter().map(|s| s.len()).sum::<usize>() + self.remaining.len();
        if total_len != raw_input.len() || !raw_input.is_ascii() {
            return None;
        }
        let mut parts = Vec::with_capacity(self.syllables.len() + 1);
        let mut pos = 0usize;
        for syl in &self.syllables {
            let next_pos = pos + syl.len();
            parts.push(&raw_input[pos..next_pos]);
            pos = next_pos;
        }
        if !self.remaining.is_empty() {
            parts.push(&raw_input[pos..]);
        }
        Some(parts.join("'"))
    }
}

/// 现代汉语普通话标准声母列表（双字母翘舌声母排在最前优先匹配）
pub static STANDARD_INITIALS: &[&str] = &[
    "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r",
    "z", "c", "s", "y", "w",
];
