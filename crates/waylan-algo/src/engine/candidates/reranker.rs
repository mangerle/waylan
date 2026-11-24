//! 多维特征线性重排器 (Feature-based Linear Reranker)
//!
//! # 设计原理
//! - **实现初衷**：替代单一依赖 `CandidateTier` 枚举档位与零散手工阈值的排序方式，
//!   在保护专名与模式分流的前提下，提取覆盖率、词长、语言模型基础分、中英混输、
//!   规则归一与容错标记等 16 维特征进行统一线性打分，并提供 Pairwise 权重训练接口。
//! - **核心优势**：排序逻辑可量化、可解释且支持基于 `eval_bench` 评测样本离线校准权重。
//! - **代价与局限**：线性模型无法直接表达特征高阶交叉项，复杂语义仍依赖前端解码与 NNLM 重排。

use serde::{Deserialize, Serialize};

use crate::types::{AlgoCandidate, CandidateTier};

/// 候选词 16 维重排特征向量
#[derive(Debug, Clone, Copy, Default)]
pub struct RerankFeatures {
    /// 输入字节覆盖率（`consumed_len / raw_len`）
    pub coverage_ratio: f64,
    /// 是否完整覆盖全输入（1.0 或 0.0）
    pub is_full_coverage: f64,
    /// 是否精确整词档位
    pub is_exact_word: f64,
    /// 是否用户历史记忆档位
    pub is_user_history: f64,
    /// 是否规则归一/儿化音档位
    pub is_rule_norm: f64,
    /// 是否整句解码档位
    pub is_sentence: f64,
    /// 是否纯简拼档位
    pub is_acronym: f64,
    /// 是否容错纠错档位
    pub is_typo: f64,
    /// 是否局部前缀档位
    pub is_partial: f64,
    /// 归一化字符长度（封顶 16 字符）
    pub char_len_norm: f64,
    /// 归一化基础解码分
    pub base_score_norm: f64,
    /// 是否为多字词组
    pub is_multi_char_phrase: f64,
    /// 是否包含 ASCII 英文混输片段
    pub has_english_mix: f64,
    /// 是否为单字候选
    pub is_single_char: f64,
    /// 是否带有容错注释标记
    pub has_typo_comment: f64,
    /// 是否带有混拼注释标记
    pub has_mixed_comment: f64,
}

impl RerankFeatures {
    /// 从候选词与原始输入提取 16 维特征
    pub fn extract(raw_input: &str, cand: &AlgoCandidate) -> Self {
        let raw_len = raw_input.len().max(1) as f64;
        let consumed = cand.consumed_len.unwrap_or(raw_input.len()) as f64;
        let coverage_ratio = (consumed / raw_len).clamp(0.0, 1.0);
        let is_full_coverage = if cand.consumed_len.unwrap_or(raw_input.len()) >= raw_input.len() {
            1.0
        } else {
            0.0
        };
        let char_len = cand.text.chars().count();
        let has_ascii = cand.text.bytes().any(|b| b.is_ascii_alphabetic());
        let comment = cand.comment.as_deref().unwrap_or("");

        Self {
            coverage_ratio,
            is_full_coverage,
            is_exact_word: flag(cand.tier == CandidateTier::ExactWord),
            is_user_history: flag(cand.tier == CandidateTier::UserHistory),
            is_rule_norm: flag(cand.tier == CandidateTier::RuleNormalization),
            is_sentence: flag(cand.tier == CandidateTier::SentenceViterbi),
            is_acronym: flag(cand.tier == CandidateTier::Acronym),
            is_typo: flag(cand.tier == CandidateTier::TypoCorrection),
            is_partial: flag(cand.tier == CandidateTier::PrefixPartial),
            char_len_norm: (char_len.min(16) as f64) / 16.0,
            base_score_norm: ((cand.score as f64).min(2_000_000.0)) / 100_000.0,
            is_multi_char_phrase: flag(char_len >= 2),
            has_english_mix: flag(has_ascii),
            is_single_char: flag(char_len == 1),
            has_typo_comment: flag(
                cand.typo_count > 0
                    || cand.tier == CandidateTier::TypoCorrection
                    || comment.contains("(纠错"),
            ),
            has_mixed_comment: flag(
                cand.tier == CandidateTier::SentenceViterbi
                    && cand.typo_count == 0
                    && cand.comment.is_some()
                    || comment.contains("(混拼"),
            ),
        }
    }

    /// 转换为固定顺序的 16 维数组，便于向量点积与梯度更新
    #[inline]
    pub fn to_array(self) -> [f64; 16] {
        [
            self.coverage_ratio,
            self.is_full_coverage,
            self.is_exact_word,
            self.is_user_history,
            self.is_rule_norm,
            self.is_sentence,
            self.is_acronym,
            self.is_typo,
            self.is_partial,
            self.char_len_norm,
            self.base_score_norm,
            self.is_multi_char_phrase,
            self.has_english_mix,
            self.is_single_char,
            self.has_typo_comment,
            self.has_mixed_comment,
        ]
    }
}

#[inline]
fn flag(cond: bool) -> f64 {
    if cond { 1.0 } else { 0.0 }
}

/// 16 维线性重排权重参数（支持序列化持久化与 Pairwise 训练校准）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankWeights {
    pub weights: [f64; 16],
}

impl Default for RerankWeights {
    fn default() -> Self {
        Self {
            weights: [
                2.5,  // coverage_ratio
                3.0,  // is_full_coverage
                6.0,  // is_exact_word
                5.5,  // is_user_history
                4.0,  // is_rule_norm
                3.5,  // is_sentence
                1.5,  // is_acronym
                0.8,  // is_typo
                -1.5, // is_partial
                1.2,  // char_len_norm
                1.8,  // base_score_norm
                1.0,  // is_multi_char_phrase
                0.4,  // has_english_mix
                -0.6, // is_single_char
                -1.2, // has_typo_comment
                -0.8, // has_mixed_comment
            ],
        }
    }
}

impl RerankWeights {
    /// 计算特征向量的线性综合得分
    #[inline]
    pub fn score(&self, feat: &RerankFeatures) -> f64 {
        let arr = feat.to_array();
        arr.iter()
            .zip(self.weights.iter())
            .map(|(x, w)| x * w)
            .sum()
    }
}

#[cfg(test)]
impl RerankWeights {
    /// 基于 Pairwise Logistic 损失执行梯度下降，从 `(正例特征, 负例特征)` 样本对校准权重
    pub fn train_pairwise(
        &mut self,
        pairs: &[(RerankFeatures, RerankFeatures)],
        learning_rate: f64,
        epochs: usize,
    ) {
        if pairs.is_empty() || epochs == 0 {
            return;
        }
        for _ in 0..epochs {
            for (pos, neg) in pairs {
                let p_arr = pos.to_array();
                let n_arr = neg.to_array();
                let diff_score = self.score(pos) - self.score(neg);
                // sigmoid(-diff_score) = 1 / (1 + exp(diff_score))
                let grad_scale = 1.0 / (1.0 + diff_score.clamp(-20.0, 20.0).exp());
                for i in 0..16 {
                    self.weights[i] += learning_rate * grad_scale * (p_arr[i] - n_arr[i]);
                }
            }
        }
    }
}

/// 候选词多维特征重排器
pub struct CandidateReranker;

impl CandidateReranker {
    /// 使用默认线性特征权重对非专名候选词执行综合评分校准
    ///
    /// 保留 `ProtectedEntity` 专名置顶与各档位原始语义标签，
    /// 仅对普通候选在同一档位及全覆盖排序边界内注入多维线性特征增益。
    pub fn rerank_with_default(raw_input: &str, candidates: &mut [AlgoCandidate]) {
        let weights = RerankWeights::default();
        Self::rerank_with_weights(raw_input, candidates, &weights);
    }

    /// 使用指定线性权重校准候选词综合得分
    pub fn rerank_with_weights(
        raw_input: &str,
        candidates: &mut [AlgoCandidate],
        weights: &RerankWeights,
    ) {
        if raw_input.is_empty() || candidates.len() <= 1 {
            return;
        }
        for cand in candidates.iter_mut() {
            if cand.tier == CandidateTier::ProtectedEntity {
                continue;
            }
            let feat = RerankFeatures::extract(raw_input, cand);
            let linear_score = weights.score(&feat);
            // 将线性特征分映射为温和但可区分的正向校准分，叠加到基础分之上
            let scaled = (linear_score * 500.0).round() as i64;
            let adjusted = (cand.score as i64).saturating_add(scaled).max(1);
            cand.score = (adjusted as u64).min(u32::MAX as u64 / 4) as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rerank_features_and_pairwise_training() {
        let pos_cand = AlgoCandidate::new("我想去google", "wo'xiang'qu'google", 120_000)
            .with_tier(CandidateTier::SentenceViterbi)
            .with_consumed_len(15);
        let neg_cand = AlgoCandidate::new("我想", "wo'xiang", 120_000)
            .with_tier(CandidateTier::PrefixPartial)
            .with_consumed_len(7);

        let pos_feat = RerankFeatures::extract("woxiangqugoogle", &pos_cand);
        let neg_feat = RerankFeatures::extract("woxiangqugoogle", &neg_cand);
        let mut weights = RerankWeights::default();
        assert!(weights.score(&pos_feat) > weights.score(&neg_feat));

        let before_diff = weights.score(&pos_feat) - weights.score(&neg_feat);
        weights.train_pairwise(&[(pos_feat, neg_feat)], 0.05, 5);
        let after_diff = weights.score(&pos_feat) - weights.score(&neg_feat);
        assert!(after_diff > before_diff);
    }
}
