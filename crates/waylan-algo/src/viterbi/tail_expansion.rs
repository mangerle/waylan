//! 维特比整句解码末尾未完成拼音与声母链扩展
//!
//! # 设计原理
//! - **实现初衷**：将末尾残存拼音（如未输入完整的韵母、连续声母简拼）的启发式扩展与主词格动态规划解耦。
//! - **核心优势**：
//!   1. 关注点分离：将多声母简拼链 (A1)、前驱音节双字词组合 (A2) 与单字保底 (B) 拆分为高内聚子函数；
//!   2. 复杂度控制：消除原本 150 行的巨型单体方法，单函数均控制在 35 行以内；
//!   3. 零多余堆分配：复用缓冲区与反向指针拓扑回溯。

use super::decoder::{backtrack_sentence, to_positive_score, unigram_log_score, ViterbiBeamNode};
use super::language_model::BigramLanguageModel;
use crate::syllables::find_syllables_by_prefix;
use crate::table::LexiconTable;
use crate::types::{AlgoCandidate, CandidateTier};

/// 维特比尾部拼音与声母扩展上下文参数对象
pub(super) struct TailExpansionContext<'a> {
    pub syllables: &'a [String],
    pub remaining: &'a str,
    pub table: &'a LexiconTable,
    pub has_bigram: bool,
    pub prev_context: Option<&'a str>,
    pub enable_acronym: bool,
}

/// 候选分支扩展项 (回溯步索引, 束索引, 扩展词文本, 累计得分)
type ExtensionBranch = (usize, usize, String, i64);

/// 单个双字词条目向前驱束集合扩展的上下文参数对象
struct CompoundEntryParams<'a, 'b> {
    /// 尾部扩展上下文
    ctx: &'a TailExpansionContext<'b>,
    /// 前驱步索引
    prev_step: usize,
    /// 前驱束集合
    prev_beam: &'a [ViterbiBeamNode],
    /// 候选词文本
    text: &'a str,
    /// 候选词在二元词表中的词 ID
    word_id: Option<u32>,
    /// 候选词原始词频
    raw_score: u32,
}

/// 对末尾残存拼音与声母链进行结合扩展与整句回溯生成
pub(super) fn expand_remaining_tail(
    ctx: &TailExpansionContext<'_>,
    dp: &[Vec<ViterbiBeamNode>],
) -> Option<AlgoCandidate> {
    let mut ext_candidates: Vec<ExtensionBranch> = Vec::new();

    // 1. 回溯提取尾部声母链 initial_chain
    let (initial_chain, split_idx) = extract_initial_chain(ctx.syllables, ctx.remaining);
    let k = initial_chain.len();

    if k >= 2 {
        // 策略 A1：多声母连续简拼链绑定（受用户配置控制）
        if ctx.enable_acronym {
            expand_acronym_chain(ctx, &initial_chain, split_idx, dp, &mut ext_candidates);
        }
    } else if split_idx >= 1 && !dp[split_idx - 1].is_empty() {
        // 策略 A2：k = 1 单声母未完成态，与倒数第 1 个完整音节结合扩展为双字词
        expand_compound_with_prefix(ctx, split_idx, dp, &mut ext_candidates);
    }

    // 策略 B：降级回退（仅当策略 A1/A2 均无法组合成词时，才允许单字母枚举为高频单字）
    let n = ctx.syllables.len();
    if ext_candidates.is_empty() && !dp[n].is_empty() {
        expand_fallback_initial(ctx, n, dp, &mut ext_candidates);
    }

    assemble_best_candidate(ext_candidates, dp, ctx.syllables, ctx.remaining)
}

/// 从音节切分末尾逆向收集连续声母链
fn extract_initial_chain(syllables: &[String], remaining: &str) -> (Vec<String>, usize) {
    let mut initial_chain = vec![remaining.to_string()];
    let mut split_idx = syllables.len();
    while split_idx > 0 {
        let prev_syl = &syllables[split_idx - 1];
        if !crate::syllables::is_valid_syllable(prev_syl) {
            initial_chain.insert(0, prev_syl.clone());
            split_idx -= 1;
        } else {
            break;
        }
    }
    (initial_chain, split_idx)
}

/// 策略 A1：多声母连续简拼链绑定与转移计算
fn expand_acronym_chain(
    ctx: &TailExpansionContext<'_>,
    initial_chain: &[String],
    split_idx: usize,
    dp: &[Vec<ViterbiBeamNode>],
    out: &mut Vec<ExtensionBranch>,
) {
    if dp[split_idx].is_empty() {
        return;
    }

    let acronym_key = initial_chain.concat();
    let acronym_cands = ctx.table.lookup_acronym(&acronym_key);
    for cand in acronym_cands.iter().take(3) {
        let freq_score = unigram_log_score(cand.score);
        let curr_char_len = cand.text.chars().count() as i64;
        for (prev_beam_idx, prev) in dp[split_idx].iter().enumerate() {
            let prev_word = if split_idx == 0 {
                ctx.prev_context.unwrap_or("")
            } else {
                &prev.word
            };
            let trans_score = BigramLanguageModel::transition_score_cached(
                (prev_word, prev.last_word_id),
                (&cand.text, cand.word_id, curr_char_len),
                freq_score,
                ctx.table,
            );
            out.push((
                split_idx,
                prev_beam_idx,
                cand.text.clone(),
                prev.total_score.saturating_add(trans_score),
            ));
        }
    }
}

/// 策略 A2：单声母未完成态与前序音节双字词组合
fn expand_compound_with_prefix(
    ctx: &TailExpansionContext<'_>,
    split_idx: usize,
    dp: &[Vec<ViterbiBeamNode>],
    out: &mut Vec<ExtensionBranch>,
) {
    let last_syl = &ctx.syllables[split_idx - 1];
    let prev_step = split_idx - 1;
    let mut key_buf = String::with_capacity(16);

    for &syl in find_syllables_by_prefix(ctx.remaining) {
        build_compound_key(last_syl, syl, &mut key_buf);

        if let Some(entries) = ctx.table.get_top_raw_entries(&key_buf, 2) {
            for (text, raw_score, word_id_opt) in entries.iter().take(2) {
                push_compound_branches(
                    &CompoundEntryParams {
                        ctx,
                        prev_step,
                        prev_beam: &dp[prev_step],
                        text,
                        word_id: *word_id_opt,
                        raw_score: *raw_score,
                    },
                    out,
                );
            }
        }
    }
}

/// 拼接候选双字词的拼音键（前一完整音节 + 声母前缀展开所得音节）
fn build_compound_key(last_syl: &str, syl: &str, key_buf: &mut String) {
    key_buf.clear();
    key_buf.push_str(last_syl);
    key_buf.push('\'');
    key_buf.push_str(syl);
}

/// 将单个双字词条目与前驱束集合组合为扩展分支并推入结果
fn push_compound_branches(params: &CompoundEntryParams<'_, '_>, out: &mut Vec<ExtensionBranch>) {
    let freq_score = unigram_log_score(params.raw_score);
    let curr_word_id = params.word_id.or_else(|| {
        if params.ctx.has_bigram {
            params.ctx.table.get_word_id(params.text)
        } else {
            None
        }
    });
    let curr_char_len = params.text.chars().count() as i64;

    for (prev_beam_idx, prev) in params.prev_beam.iter().enumerate() {
        let prev_word = if params.prev_step == 0 {
            params.ctx.prev_context.unwrap_or("")
        } else {
            &prev.word
        };
        let trans_score = BigramLanguageModel::transition_score_cached(
            (prev_word, prev.last_word_id),
            (params.text, curr_word_id, curr_char_len),
            freq_score,
            params.ctx.table,
        );
        out.push((
            params.prev_step,
            prev_beam_idx,
            params.text.to_string(),
            prev.total_score.saturating_add(trans_score),
        ));
    }
}

/// 策略 B：声母降级回退至高频单字
fn expand_fallback_initial(
    ctx: &TailExpansionContext<'_>,
    n: usize,
    dp: &[Vec<ViterbiBeamNode>],
    out: &mut Vec<ExtensionBranch>,
) {
    let Some(entries) = ctx.table.get_initial_raw_entries(ctx.remaining) else {
        return;
    };

    for (text, raw_score, word_id_opt) in entries.iter().take(3) {
        let freq_score = unigram_log_score(*raw_score);
        let curr_word_id = word_id_opt.or_else(|| {
            if ctx.has_bigram {
                ctx.table.get_word_id(text)
            } else {
                None
            }
        });
        let curr_char_len = text.chars().count() as i64;

        for (prev_beam_idx, prev) in dp[n].iter().enumerate() {
            let prev_word = if n == 0 {
                ctx.prev_context.unwrap_or("")
            } else {
                &prev.word
            };
            let trans_score = BigramLanguageModel::transition_score_cached(
                (prev_word, prev.last_word_id),
                (text, curr_word_id, curr_char_len),
                freq_score,
                ctx.table,
            );
            out.push((
                n,
                prev_beam_idx,
                text.clone(),
                prev.total_score.saturating_add(trans_score),
            ));
        }
    }
}

/// 从所有扩展分支中选择全局最优候选并逆向回溯拼接整句
fn assemble_best_candidate(
    ext_candidates: Vec<ExtensionBranch>,
    dp: &[Vec<ViterbiBeamNode>],
    syllables: &[String],
    remaining: &str,
) -> Option<AlgoCandidate> {
    let (best_step, best_beam_idx, best_word, best_score) =
        ext_candidates.into_iter().max_by_key(|cand| cand.3)?;

    let full_text = backtrack_sentence(dp, best_step, best_beam_idx, Some(&best_word));
    let pinyin_str = if syllables.is_empty() {
        remaining.to_string()
    } else {
        format!("{}'{}", syllables.join("'"), remaining)
    };
    let positive_score = to_positive_score(best_score);

    Some(
        AlgoCandidate::new(full_text, pinyin_str, positive_score)
            .with_comment("整句预测")
            .with_tier(CandidateTier::SentenceViterbi),
    )
}
