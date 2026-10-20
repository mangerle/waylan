use super::fuzzy::PrismFuzzy;
use crate::syllables::is_valid_syllable;
use crate::types::FuzzyConfig;

/// 音节图容错虚边探索强度
///
/// # 设计原理
/// - **实现初衷**：容错虚边包含两类性质完全不同的改写。一类是汉语拼音方案的**确定性省写归一**
///   （`iou`/`uei`/`uen` 带声母时须写作 `iu`/`ui`/`un`，`v` 与 `ü` 两套键位，`ong` 漏写尾辅音等），
///   命中即为正解、无歧义；另一类是**概率性物理误触猜测**（QWERTY 邻键替换、相邻字母倒置），
///   命中与否取决于误触概率。原实现用单个 `enable_typo: bool` 同时控制两类，并与"正常击键流"
///   判定耦合，导致 `niou`、`guei`、`luen` 这类**能完整切分但写法非标准**的输入被判定为正常流后
///   彻底跳过容错搜索，确定性归一规则在正常打字路径上永远无法执行。
/// - **核心优势**：正常击键流仅追加确定性归一虚边，其开销为常数字节的查表与字符串改写，
///   不含编辑距离动态规划，且被 `fsa_matches` 长匹配守卫进一步剪枝；概率性猜测仍只在异常流启用，
///   避免正常打字被邻键噪声污染。
/// - **代价与局限**：正常流每次击键的 DAG 分支数增加，候选总量随之上升；对 `lv` 这类本身合法
///   却同时是规则左值的输入，会额外召回 `lu` 系候选（属规则声明的预期行为）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypoExploration {
    /// 严格模式：不生成任何容错虚边（纯声母缩写与主开关关闭时使用）
    Strict,
    /// 确定性归一：仅应用拼音方案改写规则，不含物理邻键替换与字母倒置猜测
    RuleOnly,
    /// 全量容错：拼音方案改写规则 + 物理邻键替换 + 相邻字母倒置
    Full,
}

impl TypoExploration {
    /// 是否允许探索拼音方案确定性改写虚边
    #[inline]
    pub fn allows_rule(self) -> bool {
        !matches!(self, Self::Strict)
    }

    /// 是否允许探索概率性物理误触虚边（邻键替换与相邻字母倒置）
    #[inline]
    pub fn allows_spatial(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// 音节切分路径的来源性质
///
/// # 设计原理
/// - **实现初衷**：原契约用 `is_typo: bool` 表达"是否容错衍生"，但容错虚边本身分确定性归一与
///   概率性误触两档（见 [`TypoExploration`]）。布尔量无法把这一差异传递给候选收集层，导致可信度
///   完全不同的两类候选被压进同一个排序层级，`guei` 的"贵"只能排在 `股诶` 这类生造整句之后。
/// - **核心优势**：变体声明顺序即强弱顺序（派生 `Ord`），"既是归一又是邻键"这类非法组合无法表示；
///   收集层据此把归一候选排在概率猜测之上、整句解码之下。
/// - **代价与局限**：同一条路径混用两类虚边时只能记录较弱的一档（取 `max`），归一收益因此偏保守。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathOrigin {
    /// 严格按标准音节切分得到，未经任何容错
    Exact,
    /// 由拼音方案确定性改写规则归一得到（`iou`→`iu`、`on`→`ong` 等）
    RuleNormalized,
    /// 由物理邻键替换或相邻字母倒置等概率性猜测得到
    SpatialTypo,
}

/// 完整音节切分路径元数据契约
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPath {
    /// 完整音节切分列表（如 ["xi", "an"] 或 ["xian"]）
    pub syllables: Vec<String>,
    /// 末尾未完成键入的剩余前缀（如 "h"）
    pub remaining: String,
    /// 路径代价值（段数越少、匹配越长代价越低，优先排序）
    pub cost: u32,
    /// 路径来源性质（严格切分 / 确定性归一 / 概率性容错）
    pub origin: PathOrigin,
}

impl SegmentPath {
    /// 该路径是否经容错虚边（确定性归一或概率性猜测）衍生
    #[inline]
    pub fn is_typo(&self) -> bool {
        self.origin != PathOrigin::Exact
    }
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
        if self.is_typo() {
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
