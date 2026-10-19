//! 维特比整句动态规划解码器 (Viterbi Beam Search Decoder)

use super::language_model::BigramLanguageModel;
use super::tail_expansion::expand_remaining_tail;
use crate::table::LexiconTable;
use crate::types::{AlgoCandidate, CandidateTier};

/// 束搜索动态规划格点状态（引入反向指针拓扑与词 ID 缓存，彻底消除中间路径字符串克隆与重复查表）
#[derive(Debug, Clone)]
pub(super) struct ViterbiBeamNode {
    /// 当前步转移的构词文本
    pub(super) word: String,
    /// 当前词在 Bigram 词表中的整数 ID 缓存
    pub(super) last_word_id: Option<u32>,
    /// 前驱步在 dp 中的索引（0 表示虚拟根节点）
    pub(super) prev_step: usize,
    /// 前驱节点在 dp[prev_step] 束集合中的下标
    pub(super) prev_beam_idx: usize,
    /// 累计对数语言模型得分
    pub(super) total_score: i64,
}

/// 沿反向指针逆向回溯拼装整句文本，仅在选出全局最优解时触发一次聚合
pub(super) fn backtrack_sentence(
    dp: &[Vec<ViterbiBeamNode>],
    mut step: usize,
    mut beam_idx: usize,
    tail_word: Option<&str>,
) -> String {
    let mut words = Vec::new();
    if let Some(tail) = tail_word {
        words.push(tail);
    }
    while step > 0 {
        let node = &dp[step][beam_idx];
        words.push(&node.word);
        step = node.prev_step;
        beam_idx = node.prev_beam_idx;
    }
    words.reverse();
    words.concat()
}

/// 维特比整句动态规划解码器 (Viterbi Beam Search Decoder)
///
/// # 设计原理
/// - 在构建的标准 `WordLattice` 拓扑上，利用 `BigramLanguageModel` 的转移边权；
/// - 通过束搜索（Top-8 保留）在微秒内求解全局联合似然最高的整句路径。
#[derive(Debug, Default)]
pub struct ViterbiDecoder;

impl ViterbiDecoder {
    /// 默认基础束搜索最大保留宽度
    pub const DEFAULT_BEAM_WIDTH: usize = 8;

    /// 依据音节切分序列长度自适应计算束搜索保留宽度
    ///
    /// # 算法策略
    /// - 短词短语 (音节数 <= 4): 束宽 8，保持微秒级超低延迟；
    /// - 中等短语 (音节数 5..=8): 束宽 12，平衡探索广度与计算量；
    /// - 长句 (音节数 9..=15): 束宽 16，保留更多弱局部假设以保障全局连贯性；
    /// - 超长句 (音节数 > 15): 束宽 20，彻底解决前序贪心激进剪枝导致的长句失步问题。
    pub fn dynamic_beam_width(syllable_count: usize) -> usize {
        if syllable_count <= 4 {
            Self::DEFAULT_BEAM_WIDTH
        } else if syllable_count <= 8 {
            12
        } else if syllable_count <= 15 {
            16
        } else {
            20
        }
    }

    /// 对给定的音节切分列表与末尾残存拼音进行整句动态规划求解
    pub fn decode_sentence(
        syllables: &[String],
        remaining: &str,
        table: &LexiconTable,
    ) -> Option<AlgoCandidate> {
        Self::decode_sentence_with_context(syllables, remaining, table, None)
    }

    /// 对给定的音节切分列表、末尾残存拼音与上屏历史上下文进行整句动态规划求解
    pub fn decode_sentence_with_context(
        syllables: &[String],
        remaining: &str,
        table: &LexiconTable,
        prev_context: Option<&str>,
    ) -> Option<AlgoCandidate> {
        let n = syllables.len();
        if n == 0 && remaining.is_empty() {
            return None;
        }

        let has_bigram = table.compact_bigram.is_some();
        let ctx_str = prev_context.unwrap_or("");
        let ctx_word_id = if has_bigram && !ctx_str.is_empty() {
            table.get_word_id(ctx_str)
        } else {
            None
        };

        // dp[i] 保存消耗了前 i 个完整音节时的最优束状态集合
        let mut dp: Vec<Vec<ViterbiBeamNode>> = vec![Vec::new(); n + 1];

        // 虚拟起始根节点
        dp[0].push(ViterbiBeamNode {
            word: String::new(),
            last_word_id: ctx_word_id,
            prev_step: 0,
            prev_beam_idx: 0,
            total_score: 0,
        });

        // 沿音节序列向右推进求解 Viterbi 最优路径
        Self::solve_syllable_lattice(syllables, table, has_bigram, ctx_str, &mut dp);

        // 末尾残缺音节与声母扩展（处理未完成键入态）
        if !remaining.is_empty() {
            if let Some(cand) =
                expand_remaining_tail(syllables, remaining, table, has_bigram, prev_context, &dp)
            {
                return Some(cand);
            }
        }

        // 若无 remaining 或扩展未命中，直接提取到达第 n 个音节的全局最优解
        if dp[n].is_empty() {
            return None;
        }

        let best = &dp[n][0];
        let full_text = backtrack_sentence(&dp, n, 0, None);
        let pinyin_str = syllables.join("'");
        let positive_score = to_positive_score(best.total_score);

        Some(
            AlgoCandidate::new(full_text, pinyin_str, positive_score)
                .with_comment("整句预测")
                .with_tier(CandidateTier::SentenceViterbi),
        )
    }

    /// 遍历音节切分列表并在词格点拓扑上推进束搜索动态规划
    fn solve_syllable_lattice(
        syllables: &[String],
        table: &LexiconTable,
        has_bigram: bool,
        ctx_str: &str,
        dp: &mut [Vec<ViterbiBeamNode>],
    ) {
        let n = syllables.len();
        let beam_width = Self::dynamic_beam_width(n);
        let raw_limit = if n > 4 { 6 } else { 4 };

        // 预分配复用 key 拼接缓冲区，消除 span > 1 时的临时 String 分配
        let mut key_buf = String::with_capacity(32);

        for start_idx in 0..n {
            if dp[start_idx].is_empty() {
                continue;
            }

            let max_span = (n - start_idx).min(5);

            for span in 1..=max_span {
                let end_idx = start_idx + span;
                let sub_syls = &syllables[start_idx..end_idx];

                let key: &str = if span == 1 {
                    sub_syls[0].as_str()
                } else {
                    key_buf.clear();
                    for (i, syl) in sub_syls.iter().enumerate() {
                        if i > 0 {
                            key_buf.push('\'');
                        }
                        key_buf.push_str(syl);
                    }
                    &key_buf
                };

                let raw_entries = match table.get_top_raw_entries(key, raw_limit) {
                    Some(entries) if !entries.is_empty() => entries,
                    _ => {
                        if span == 1 {
                            if let Some(entries) = table.get_initial_raw_entries(key) {
                                entries
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    }
                };

                let (left_dp, right_dp) = dp.split_at_mut(end_idx);
                let prev_beam = &left_dp[start_idx];
                let target_beam = &mut right_dp[0];

                for (text, raw_score, word_id_opt) in raw_entries.iter().take(raw_limit) {
                    let freq_score = unigram_log_score(*raw_score);
                    let curr_word_id = word_id_opt.or_else(|| {
                        if has_bigram {
                            table.get_word_id(text)
                        } else {
                            None
                        }
                    });
                    let curr_char_len = text.chars().count() as i64;

                    let mut best_prev_idx = 0usize;
                    let mut best_total = i64::MIN;

                    for (prev_beam_idx, prev_node) in prev_beam.iter().enumerate() {
                        let prev_word = if start_idx == 0 {
                            ctx_str
                        } else {
                            prev_node.word.as_str()
                        };
                        let trans_score = BigramLanguageModel::transition_score_cached(
                            (prev_word, prev_node.last_word_id),
                            (text, curr_word_id, curr_char_len),
                            freq_score,
                            table,
                        );
                        let candidate_total = prev_node.total_score.saturating_add(trans_score);
                        if candidate_total > best_total {
                            best_total = candidate_total;
                            best_prev_idx = prev_beam_idx;
                        }
                    }

                    if best_total > i64::MIN {
                        if let Some(existing) =
                            target_beam.iter_mut().find(|node| node.word == *text)
                        {
                            if best_total > existing.total_score {
                                existing.prev_step = start_idx;
                                existing.prev_beam_idx = best_prev_idx;
                                existing.total_score = best_total;
                            }
                        } else {
                            target_beam.push(ViterbiBeamNode {
                                word: text.clone(),
                                last_word_id: curr_word_id,
                                prev_step: start_idx,
                                prev_beam_idx: best_prev_idx,
                                total_score: best_total,
                            });
                        }
                    }
                }

                // 束搜索剪枝：保留得分最高的 Top-K 个不同末端词状态（根据序列长度自适应确定 K）
                target_beam.sort_by_key(|state| std::cmp::Reverse(state.total_score));
                target_beam.truncate(beam_width);
            }
        }
    }
}

/// 将原始静态词频转换为与二元转移矩阵同量纲的对数似然得分
#[inline]
pub(super) fn unigram_log_score(raw_score: u32) -> i64 {
    ((raw_score as f64 + 1.0).ln() * 1_200.0) as i64
}

/// 将 Viterbi 路径累计对数似然安全映射为正整数候选分
#[inline]
pub(super) fn to_positive_score(total_score: i64) -> u32 {
    total_score
        .saturating_add(100_000)
        .clamp(1, u32::MAX as i64) as u32
}
