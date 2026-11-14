//! 轻量级 2-gram 统计语言模型与平滑算法 (Bigram Language Model)

use super::weights::ScoringWeights;
use crate::table::LexiconTable;

/// 轻量级 2-gram 统计语言模型与平滑算法 (Bigram Language Model)
///
/// # 设计原理
/// - **实现初衷**：替换原有硬编码的分词惩罚与静态长度激励，由相邻二元词对转移概率直接主导路径寻优；
/// - **核心优势**：根除长句输入时同音词搭配失调（如“清华大雪” vs “清华大学”）；
/// - **平滑机制**：未命中显式二元搭配时自动回退至单字似然（Back-off），保证高鲁棒性。
#[derive(Debug, Default)]
pub struct BigramLanguageModel;

impl BigramLanguageModel {
    /// 切词惩罚基准
    pub const BASE_WORD_PENALTY: i64 = 12_000;

    /// 计算从前驱词 `prev_word` 转移到当前词 `curr_word` 的联合对数似然得分
    ///
    /// 优先从 `table` 的零拷贝 Bigram 转移矩阵中获取百万级大语料统计亲和度；
    /// 若未命中或未挂载，平滑回退（Back-off）至基于规则的通用接续奖励。
    pub fn transition_score(
        prev_word: &str,
        curr_word: &str,
        curr_unigram: i64,
        table: &LexiconTable,
    ) -> i64 {
        let prev_id = if !prev_word.is_empty() && table.compact_bigram.is_some() {
            table.get_word_id(prev_word)
        } else {
            None
        };
        let curr_id = if !prev_word.is_empty() && table.compact_bigram.is_some() {
            table.get_word_id(curr_word)
        } else {
            None
        };
        let char_len = curr_word.chars().count() as i64;
        Self::transition_score_cached(
            (prev_word, prev_id),
            (curr_word, curr_id, char_len),
            curr_unigram,
            table,
        )
    }

    /// 使用预解析的词表 ID 与字符长度计算状态转移得分，消除内层循环重复查表与字符遍历
    #[inline]
    pub fn transition_score_cached(
        prev: (&str, Option<u32>),
        curr: (&str, Option<u32>, i64),
        curr_unigram: i64,
        table: &LexiconTable,
    ) -> i64 {
        Self::transition_score_with_weights(
            prev,
            curr,
            curr_unigram,
            (table, &ScoringWeights::default()),
        )
    }

    /// 基于可配置权重参数计算二元转移对数得分
    #[inline]
    pub fn transition_score_with_weights(
        prev: (&str, Option<u32>),
        curr: (&str, Option<u32>, i64),
        curr_unigram: i64,
        context: (&LexiconTable, &ScoringWeights),
    ) -> i64 {
        let (table, weights) = context;
        let (prev_word, prev_id) = prev;
        let (curr_word, curr_id, curr_char_len) = curr;
        let length_bonus = (curr_char_len - 1).saturating_mul(weights.length_bonus_per_char);

        if prev_word.is_empty() {
            return curr_unigram + length_bonus - weights.base_word_penalty;
        }

        let bigram_bonus = match (&table.compact_bigram, prev_id, curr_id) {
            (Some(bg), Some(pid), Some(cid)) => bg
                .get_affinity_by_id(pid, cid)
                .unwrap_or_else(|| Self::estimate_bigram_affinity(prev_word, curr_word)),
            _ => Self::estimate_bigram_affinity(prev_word, curr_word),
        };

        curr_unigram + length_bonus + bigram_bonus - weights.base_word_penalty
    }

    /// 估算二元词对的接续亲和度（未挂载大语料模型时的平滑回退）
    pub fn estimate_bigram_affinity(prev: &str, curr: &str) -> i64 {
        for &(p, c, score) in FALLBACK_BIGRAM_RULES {
            if prev == p && curr == c {
                return score;
            }
        }

        // 常见虚词与助词接续模式平滑奖励
        if matches!(curr, "的" | "了" | "着" | "过" | "吗" | "呢" | "吧") {
            8_000
        } else if prev.chars().count() >= 2 && curr.chars().count() >= 2 {
            4_000
        } else {
            0
        }
    }
}

/// 未挂载外部大语料 Bigram 统计表时的静态内置回退亲和度表
///
/// # 设计原理
/// - **实现初衷**：在轻量测试环境或未部署预编译 `bigram.table.bin` 的低内存嵌入式场景下，
///   提供常见同音词消歧二元亲和度保底支撑。
/// - **核心优势**：消除散落在逻辑分支中的魔法字符串和分数值，集中声明便于审核与扩展。
/// - **代价与局限**：覆盖词对有限，完整生产环境应优先依赖百万级紧凑二进制 Bigram 语言模型。
pub const FALLBACK_BIGRAM_RULES: &[(&str, &str, i64)] = &[
    ("你好", "世界", 18_000),
    ("我们", "来了", 16_000),
    ("我们", "走", 16_000),
    ("我们", "去", 16_000),
    ("中国", "人民", 18_000),
    ("中国", "历史", 18_000),
    ("中国", "特色", 18_000),
    ("清华", "大学", 20_000),
    ("北京", "时间", 20_000),
    ("北京", "大学", 20_000),
    ("我们的", "时间", 20_000),
    ("我们的", "世界", 20_000),
    ("我们共同的", "感觉", 20_000),
    ("共同的", "感觉", 20_000),
    ("的", "时间", 18_000),
    ("的", "世界", 18_000),
    ("的", "感觉", 18_000),
    ("开发", "软件", 18_000),
    ("操作", "系统", 18_000),
    ("人工", "智能", 18_000),
    ("永远", "的", 14_000),
    ("很好", "的", 14_000),
];
