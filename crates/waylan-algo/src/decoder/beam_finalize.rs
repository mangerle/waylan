//! 束搜索解码结果的回溯、排序去重与首词段构建模块
//!
//! # 设计原理
//! - **实现初衷**：将束搜索结束后的路径回溯与多维排序汇总从核心步进循环中剥离，
//!   使 `beam.rs` 专注于位置同步解码主干。
//! - **核心优势**：结果汇总与句首词段构造职责内聚，便于独立调整排序与去重策略。
//! - **代价与局限**：依赖 `ArenaNode` 与 `spans_by_pos` 引用切片完成路径重建。

use super::beam_scoring::score_phrase_match;
use super::beam_types::{
    ArenaNode, BeamDecodeOptions, BeamState, DecodeResult, DecodedSentence, SentenceHeadSegment,
};
use super::lattice::SyllableLattice;
use super::matcher::PhraseMatch;
use super::weights::ScoringWeights;

/// 从终态束槽回溯整句并汇总整词、句首词段与最佳预编辑拼音
pub(super) fn finalize_decode_result(
    lattice: &SyllableLattice,
    end_slot: &[BeamState],
    node_arena: &[ArenaNode],
    spans_by_pos: &[Vec<PhraseMatch>],
    mut whole_words: Vec<PhraseMatch>,
    mut prefix_word_matches: Vec<PhraseMatch>,
    options: &BeamDecodeOptions<'_>,
) -> DecodeResult {
    let sentences = collect_decoded_sentences(lattice, end_slot, node_arena, spans_by_pos, options);

    whole_words.sort_by(|a, b| {
        b.is_exact_all
            .cmp(&a.is_exact_all)
            .then_with(|| a.typo_count.cmp(&b.typo_count))
            .then_with(|| a.has_initial.cmp(&b.has_initial))
            .then_with(|| {
                score_phrase_match(b, &options.weights)
                    .cmp(&score_phrase_match(a, &options.weights))
            })
    });
    dedup_phrase_matches(&mut whole_words);
    whole_words.sort_by(|a, b| {
        score_phrase_match(b, &options.weights).cmp(&score_phrase_match(a, &options.weights))
    });

    prefix_word_matches.sort_by(|a, b| {
        score_phrase_match(b, &options.weights).cmp(&score_phrase_match(a, &options.weights))
    });
    let prefix_heads = build_prefix_heads(&sentences, &prefix_word_matches, &options.weights);

    // 预编辑文本属于用户输入击键序列的视觉切分回显，唯一且排他性地由音节格依据原始按键切分产出，
    // 严禁被语义解码结果的词库标准拼音篡改覆盖（确保输入所见即所得）。
    let best_preedit = lattice.best_formatted_preedit();

    DecodeResult {
        sentences,
        whole_words,
        prefix_heads,
        best_preedit,
    }
}

/// 回溯终态束槽并收集去重后的 Top-K 整句结果
fn collect_decoded_sentences(
    lattice: &SyllableLattice,
    end_slot: &[BeamState],
    node_arena: &[ArenaNode],
    spans_by_pos: &[Vec<PhraseMatch>],
    options: &BeamDecodeOptions<'_>,
) -> Vec<DecodedSentence> {
    let mut sentences: Vec<DecodedSentence> =
        Vec::with_capacity(end_slot.len().min(options.max_sentences));

    let mut sorted_end = end_slot.to_vec();
    sorted_end.sort_by_key(|a| std::cmp::Reverse(a.score));

    let mut refs: Vec<(usize, usize)> = Vec::with_capacity(8);
    for st in sorted_end {
        refs.clear();
        let mut cursor = Some(st.node_idx);
        while let Some(idx) = cursor {
            let node = node_arena[idx];
            if let Some(wref) = node.word_ref {
                refs.push(wref);
            }
            cursor = node.parent;
        }
        refs.reverse();

        let text: String = refs
            .iter()
            .map(|&(p, idx)| spans_by_pos[p][idx].word.as_str())
            .collect();
        if sentences.iter().any(|s| s.text == text) {
            continue;
        }

        let mut pinyin = String::with_capacity(refs.len().saturating_mul(6));
        for (i, &(p, idx)) in refs.iter().enumerate() {
            if i > 0 {
                pinyin.push('\'');
            }
            pinyin.push_str(&spans_by_pos[p][idx].matched_key);
        }
        let first_word = refs
            .first()
            .map(|&(p, idx)| spans_by_pos[p][idx].word.clone())
            .unwrap_or_default();
        let first_word_pinyin = refs
            .first()
            .map(|&(p, idx)| spans_by_pos[p][idx].matched_key.to_string())
            .unwrap_or_default();

        sentences.push(DecodedSentence {
            text,
            pinyin,
            score: st.score,
            word_count: refs.len(),
            typo_count: st.typo_count,
            has_initial: st.has_initial,
            has_partial_tail: st.has_partial_tail,
            is_exact_all: st.is_exact_all,
            first_word,
            first_word_pinyin,
            first_word_end: st.first_word_end.unwrap_or(lattice.len),
        });
        if sentences.len() >= options.max_sentences {
            break;
        }
    }
    sentences
}

/// 构造句首词段候选项（用于前向截断提交）
fn build_prefix_heads(
    sentences: &[DecodedSentence],
    prefix_word_matches: &[PhraseMatch],
    weights: &ScoringWeights,
) -> Vec<SentenceHeadSegment> {
    let mut heads = Vec::with_capacity(4);
    if let Some(best) = sentences.first()
        && best.word_count >= 2
        && best.first_word_end > 0
    {
        heads.push(SentenceHeadSegment {
            text: best.first_word.clone(),
            pinyin: best.first_word_pinyin.clone(),
            consumed_len: best.first_word_end,
            score: best.score,
        });
    }
    for pm in prefix_word_matches.iter().take(3) {
        if heads.iter().any(|h| h.text == pm.word) {
            continue;
        }
        heads.push(SentenceHeadSegment {
            text: pm.word.clone(),
            pinyin: pm.matched_key.to_string(),
            consumed_len: pm.end,
            score: score_phrase_match(pm, weights),
        });
        if heads.len() >= 3 {
            break;
        }
    }
    heads
}

/// 按词面文本原地去重（保留排序靠前的高分匹配，零堆分配与克隆）
fn dedup_phrase_matches(matches: &mut Vec<PhraseMatch>) {
    let mut write_idx = 0usize;
    for read_idx in 0..matches.len() {
        let is_dup = matches[..write_idx]
            .iter()
            .any(|prev| prev.word == matches[read_idx].word);
        if !is_dup {
            if write_idx != read_idx {
                matches.swap(write_idx, read_idx);
            }
            write_idx += 1;
        }
    }
    matches.truncate(write_idx);
}
