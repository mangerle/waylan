//! 按字节位置同步的统一束搜索解码器 (Positional Beam Decoder)
//!
//! # 设计原理
//! - **实现初衷**：在按字节索引的 `SyllableLattice` 上直接运行 `beam[0..=N]` 同步束搜索，
//!   使全拼、模糊音、四类容错（最大 `max_typos = 2`）、全拼混简拼与末尾半成品补全
//!   在同一束空间内直接竞争，消除旧版多路割裂与冗余路径枚举开销。
//! - **核心优势**：支持长句多处容错、自然产出精确字节偏移的句首词段 (`consumed_len`)，
//!   并在束内直接融合 `BigramLanguageModel` 与用户词对自适应提权。
//! - **代价与局限**：受束宽 `beam_width` 截断约束，极端同音长句可能剪枝低分尾部长句。

use std::collections::HashMap;

use super::beam_cache::{
    merge_crossing_matches, CacheSnapshot, IncrementalBeamCache, MAX_PHRASE_SPAN_BYTES,
};
use super::language_model::BigramLanguageModel;
use super::lattice::SyllableLattice;
use super::matcher::{DatPhraseMatcher, MatcherOptions, PhraseMatch};
use super::weights::ScoringWeights;
use crate::table::LexiconTable;
use crate::user_adaptive::UserAdaptiveModel;

/// 句首词段候选（用于长句第 2 候选位上屏前半截并保留剩余拼音继续编辑）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentenceHeadSegment {
    /// 句首词段中文文本
    pub text: String,
    /// 句首词段对应的格式化拼音键
    pub pinyin: String,
    /// 该词段覆盖原始输入的精确 UTF-8 字节长度
    pub consumed_len: usize,
    /// 解码综合得分
    pub score: i64,
}

/// 束搜索解码产出的整句候选
#[derive(Debug, Clone)]
pub struct DecodedSentence {
    /// 整句中文文本
    pub text: String,
    /// 整句格式化拼音键（音节间以 `'` 连接）
    pub pinyin: String,
    /// 路径总对数得分
    pub score: i64,
    /// 组成该句子的词数
    pub word_count: usize,
    /// 路径累计消耗的容错次数
    pub typo_count: u8,
    /// 是否包含混简拼展开
    pub has_initial: bool,
    /// 是否包含末尾半成品补全
    pub has_partial_tail: bool,
    /// 是否全路径均为精确全拼
    pub is_exact_all: bool,
    /// 首词文本
    pub first_word: String,
    /// 首词拼音键
    pub first_word_pinyin: String,
    /// 首词结束的原始输入字节长度
    pub first_word_end: usize,
}

/// 统一束搜索解码结果包
#[derive(Debug, Clone, Default)]
pub struct DecodeResult {
    /// 完整覆盖全输入的 Top-K 整句候选
    pub sentences: Vec<DecodedSentence>,
    /// 完整覆盖 `0..N` 的整词匹配（按综合词分降序）
    pub whole_words: Vec<PhraseMatch>,
    /// 提取的句首词段候选列表
    pub prefix_heads: Vec<SentenceHeadSegment>,
    /// 最优格式化预编辑拼音串
    pub best_preedit: String,
}

/// 束搜索解码参数对象
#[derive(Debug, Clone)]
pub struct BeamDecodeOptions<'a> {
    /// 每个字节位置保留的束宽（默认 24）
    pub beam_width: usize,
    /// 单条路径允许的最大容错次数（默认 2）
    pub max_typos: u8,
    /// 输出的最大整句候选数（默认 8）
    pub max_sentences: usize,
    /// DAT 词组匹配选项
    pub matcher_options: MatcherOptions,
    /// 评分权重
    pub weights: ScoringWeights,
    /// 当前评估时间戳（秒），用于词对时间衰减计算
    pub eval_ts: u64,
    /// 上一轮上屏历史词（为句首第一个词提供 Bigram 与用户词对上下文）
    pub last_commit_context: Option<&'a str>,
}

impl Default for BeamDecodeOptions<'_> {
    fn default() -> Self {
        Self {
            beam_width: 24,
            max_typos: 2,
            max_sentences: 8,
            matcher_options: MatcherOptions::default(),
            weights: ScoringWeights::default(),
            eval_ts: 0,
            last_commit_context: None,
        }
    }
}

/// 束搜索回溯节点（存放于连续 Arena 中，避免状态扩展时复制整条词序列）
#[derive(Debug, Clone, Copy)]
pub(super) struct ArenaNode {
    pub(super) parent: Option<usize>,
    pub(super) word_ref: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BeamState {
    pub(super) node_idx: usize,
    pub(super) score: i64,
    pub(super) typo_count: u8,
    pub(super) has_initial: bool,
    pub(super) has_partial_tail: bool,
    pub(super) is_exact_all: bool,
    pub(super) word_count: usize,
    pub(super) last_word_ref: Option<(usize, usize)>,
    pub(super) last_word_id: Option<u32>,
    pub(super) first_word_end: Option<usize>,
}

/// 按字节位置同步的统一束搜索解码器
pub struct PositionalBeamDecoder;

impl PositionalBeamDecoder {
    /// 在统一音节格上执行端到端束搜索解码（无缓存全量路径）
    pub fn decode(
        lattice: &SyllableLattice,
        table: &LexiconTable,
        adaptive: Option<&UserAdaptiveModel>,
        options: &BeamDecodeOptions<'_>,
    ) -> DecodeResult {
        Self::decode_core(lattice, table, adaptive, options, None, 0)
    }

    /// 在统一音节格上执行支持前缀增量缓存的端到端束搜索解码
    pub fn decode_with_cache(
        lattice: &SyllableLattice,
        table: &LexiconTable,
        adaptive: Option<&UserAdaptiveModel>,
        options: &BeamDecodeOptions<'_>,
        cache: &mut IncrementalBeamCache,
        config_sig: u64,
    ) -> DecodeResult {
        Self::decode_core(lattice, table, adaptive, options, Some(cache), config_sig)
    }

    fn decode_core(
        lattice: &SyllableLattice,
        table: &LexiconTable,
        adaptive: Option<&UserAdaptiveModel>,
        options: &BeamDecodeOptions<'_>,
        mut cache: Option<&mut IncrementalBeamCache>,
        config_sig: u64,
    ) -> DecodeResult {
        let len = lattice.len;
        if len == 0 {
            return DecodeResult::default();
        }

        let seed_word = options.last_commit_context.unwrap_or("");
        let seed_word_id = (!seed_word.is_empty())
            .then(|| table.get_word_id(seed_word))
            .flatten();

        let start_pos = lattice.skip_separators(0);
        if start_pos >= len {
            return DecodeResult::default();
        }

        let mut whole_words: Vec<PhraseMatch> = Vec::new();
        let mut prefix_word_matches: Vec<PhraseMatch> = Vec::new();

        let freeze_opt = cache
            .as_deref()
            .and_then(|c| c.compute_safe_freeze(lattice, options, config_sig))
            .filter(|&f| f > start_pos);

        let (
            mut beam,
            mut node_arena,
            mut arena_len_before_pos,
            mut spans_by_pos,
            mut pos_max_reach,
            loop_start,
        ) = if let Some(freeze_pos) = freeze_opt {
            let c = cache.as_deref_mut().expect("cache checked above");
            let mut beam = vec![Vec::new(); len + 1];
            for (slot, cached) in beam
                .iter_mut()
                .zip(c.beam_slots.iter_mut())
                .take(freeze_pos + 1)
            {
                *slot = std::mem::take(cached);
            }
            let mut node_arena = std::mem::take(&mut c.node_arena);
            node_arena.truncate(c.arena_len_before_pos[freeze_pos]);
            let mut arena_len_before_pos = vec![0usize; len];
            arena_len_before_pos[..freeze_pos]
                .copy_from_slice(&c.arena_len_before_pos[..freeze_pos]);
            let mut spans_by_pos = vec![Vec::new(); len];
            for (slot, cached) in spans_by_pos
                .iter_mut()
                .zip(c.spans_by_pos.iter_mut())
                .take(freeze_pos)
            {
                *slot = std::mem::take(cached);
            }
            let mut pos_max_reach = vec![0usize; len];
            pos_max_reach[..freeze_pos].copy_from_slice(&c.pos_max_reach[..freeze_pos]);

            // 仅对上次曾触及或跨越冻结边界的位置，补充跨边界匹配及扩展边
            let bridge_start = freeze_pos.saturating_sub(MAX_PHRASE_SPAN_BYTES);
            for pos in bridge_start..freeze_pos {
                if beam[pos].is_empty() || pos_max_reach[pos] < freeze_pos {
                    if pos == start_pos {
                        for m in &spans_by_pos[start_pos] {
                            if m.end < len
                                && m.syllable_count >= 2
                                && m.typo_count == 0
                                && !m.has_initial
                            {
                                prefix_word_matches.push(m.clone());
                            }
                        }
                    }
                    continue;
                }
                let (mut fresh, reach) = DatPhraseMatcher::match_from_with_reach(
                    lattice,
                    pos,
                    table,
                    &options.matcher_options,
                );
                pos_max_reach[pos] = reach;
                if pos == start_pos {
                    for m in &fresh {
                        if m.end == len {
                            whole_words.push(m.clone());
                        } else if m.syllable_count >= 2 && m.typo_count == 0 && !m.has_initial {
                            prefix_word_matches.push(m.clone());
                        }
                    }
                }
                filter_and_rank_matches(&mut fresh, len, &options.weights);
                merge_crossing_matches(&mut spans_by_pos[pos], fresh, freeze_pos);

                let cur_states = beam[pos].clone();
                let mut ctx = ExpandContext {
                    pos,
                    seed_word,
                    spans_by_pos: &spans_by_pos,
                    table,
                    adaptive,
                    options,
                    node_arena: &mut node_arena,
                    beam: &mut beam,
                };
                expand_position_states(&cur_states, &mut ctx, Some(freeze_pos));
            }
            if start_pos < bridge_start {
                for m in &spans_by_pos[start_pos] {
                    if m.end < len && m.syllable_count >= 2 && m.typo_count == 0 && !m.has_initial {
                        prefix_word_matches.push(m.clone());
                    }
                }
            }

            (
                beam,
                node_arena,
                arena_len_before_pos,
                spans_by_pos,
                pos_max_reach,
                freeze_pos,
            )
        } else {
            let mut beam: Vec<Vec<BeamState>> = vec![Vec::new(); len + 1];
            let mut node_arena: Vec<ArenaNode> = Vec::with_capacity(len.saturating_mul(16).max(64));
            let arena_len_before_pos = vec![0usize; len];
            let spans_by_pos: Vec<Vec<PhraseMatch>> = vec![Vec::new(); len];
            let pos_max_reach = vec![0usize; len];

            let root_idx = node_arena.len();
            node_arena.push(ArenaNode {
                parent: None,
                word_ref: None,
            });
            beam[start_pos].push(BeamState {
                node_idx: root_idx,
                score: 0,
                typo_count: 0,
                has_initial: false,
                has_partial_tail: false,
                is_exact_all: true,
                word_count: 0,
                last_word_ref: None,
                last_word_id: seed_word_id,
                first_word_end: None,
            });
            (
                beam,
                node_arena,
                arena_len_before_pos,
                spans_by_pos,
                pos_max_reach,
                start_pos,
            )
        };

        for pos in loop_start..len {
            arena_len_before_pos[pos] = node_arena.len();
            let next_non_sep = lattice.skip_separators(pos);
            if next_non_sep > pos {
                if !beam[pos].is_empty() {
                    let carried = std::mem::take(&mut beam[pos]);
                    beam[next_non_sep].extend(carried);
                    prune_beam_slot(
                        &mut beam[next_non_sep],
                        options.beam_width,
                        &spans_by_pos,
                        seed_word,
                    );
                }
                continue;
            }
            if beam[pos].is_empty() {
                continue;
            }

            prune_beam_slot(&mut beam[pos], options.beam_width, &spans_by_pos, seed_word);
            let (mut matches, reach) = DatPhraseMatcher::match_from_with_reach(
                lattice,
                pos,
                table,
                &options.matcher_options,
            );
            pos_max_reach[pos] = reach;
            if matches.is_empty() {
                continue;
            }

            if pos == start_pos {
                for m in &matches {
                    if m.end == len {
                        whole_words.push(m.clone());
                    } else if m.syllable_count >= 2 && m.typo_count == 0 && !m.has_initial {
                        prefix_word_matches.push(m.clone());
                    }
                }
            }

            filter_and_rank_matches(&mut matches, len, &options.weights);
            spans_by_pos[pos] = matches;

            let cur_states = beam[pos].clone();
            let mut ctx = ExpandContext {
                pos,
                seed_word,
                spans_by_pos: &spans_by_pos,
                table,
                adaptive,
                options,
                node_arena: &mut node_arena,
                beam: &mut beam,
            };
            expand_position_states(&cur_states, &mut ctx, None);
        }

        let result = finalize_decode_result(
            lattice,
            &beam[len],
            &node_arena,
            &spans_by_pos,
            whole_words,
            prefix_word_matches,
            options,
        );

        if let Some(c) = cache {
            c.update(
                &lattice.raw,
                config_sig,
                options.last_commit_context,
                CacheSnapshot {
                    beam_slots: beam,
                    node_arena,
                    arena_len_before_pos,
                    spans_by_pos,
                    pos_max_reach,
                },
            );
        }

        result
    }
}

/// 过滤低频生僻单字并按局部得分截取当前位置 Top-48 词匹配
fn filter_and_rank_matches(
    matches: &mut Vec<PhraseMatch>,
    target_len: usize,
    weights: &ScoringWeights,
) {
    let has_common_at_end: Vec<usize> = matches
        .iter()
        .filter(|o| o.freq >= 10)
        .map(|o| o.end)
        .collect();
    matches.retain(|m| {
        let is_rare_single = m.syllable_count == 1
            && m.freq < 10
            && m.word.chars().count() == 1
            && (m.start > 0 || m.end < target_len);
        if !is_rare_single {
            return true;
        }
        // 同跨度存在常用单字（freq >= 10）或词组时，隔离低频生僻单字，避免其参与组句挤占束宽
        !has_common_at_end.contains(&m.end)
    });
    matches.sort_by_key(|m| std::cmp::Reverse(score_phrase_match(m, weights)));
    matches.truncate(48);
}

struct ExpandContext<'a> {
    pos: usize,
    seed_word: &'a str,
    spans_by_pos: &'a [Vec<PhraseMatch>],
    table: &'a LexiconTable,
    adaptive: Option<&'a UserAdaptiveModel>,
    options: &'a BeamDecodeOptions<'a>,
    node_arena: &'a mut Vec<ArenaNode>,
    beam: &'a mut [Vec<BeamState>],
}

fn expand_position_states(
    cur_states: &[BeamState],
    ctx: &mut ExpandContext<'_>,
    min_next_pos: Option<usize>,
) {
    let pos = ctx.pos;
    let pos_matches = &ctx.spans_by_pos[pos];
    for state in cur_states {
        let last_word = match state.last_word_ref {
            Some((p, idx)) => ctx.spans_by_pos[p][idx].word.as_str(),
            None => ctx.seed_word,
        };

        for (idx, pm) in pos_matches.iter().enumerate() {
            if let Some(freeze) = min_next_pos {
                if pm.end <= freeze {
                    continue;
                }
            }
            if state.has_initial && pm.has_initial && state.word_count > 0 {
                continue;
            }
            let next_typo = state.typo_count.saturating_add(pm.typo_count);
            if next_typo > ctx.options.max_typos {
                continue;
            }
            let next_pos = pm.end;
            let char_len = pm.word.chars().count() as i64;
            let curr_unigram = unigram_log_score(pm.freq);
            let lm_delta = BigramLanguageModel::transition_score_with_weights(
                (last_word, state.last_word_id),
                (&pm.word, pm.word_id, char_len),
                curr_unigram,
                (ctx.table, &ctx.options.weights),
            );
            let pair_boost = ctx
                .adaptive
                .filter(|_| !last_word.is_empty())
                .map(|a| a.pair_boost_score(last_word, &pm.word, ctx.options.eval_ts))
                .unwrap_or(0);
            let edge_penalty = compute_edge_penalty(pm, &ctx.options.weights);

            let node_idx = ctx.node_arena.len();
            ctx.node_arena.push(ArenaNode {
                parent: Some(state.node_idx),
                word_ref: Some((pos, idx)),
            });

            ctx.beam[next_pos].push(BeamState {
                node_idx,
                score: state.score + lm_delta + pair_boost - edge_penalty,
                typo_count: next_typo,
                has_initial: state.has_initial || pm.has_initial,
                has_partial_tail: state.has_partial_tail || pm.has_partial_tail,
                is_exact_all: state.is_exact_all && pm.is_exact_all,
                word_count: state.word_count + 1,
                last_word_ref: Some((pos, idx)),
                last_word_id: pm.word_id,
                first_word_end: state.first_word_end.or(Some(pm.end)),
            });
        }
    }
}

fn prune_beam_slot(
    slot: &mut Vec<BeamState>,
    beam_width: usize,
    spans_by_pos: &[Vec<PhraseMatch>],
    seed_word: &str,
) {
    if slot.len() <= 1 {
        return;
    }
    slot.sort_by_key(|a| std::cmp::Reverse(a.score));

    let mut best_by_key: HashMap<(&str, u8), usize> = HashMap::with_capacity(slot.len().min(64));
    let mut kept = Vec::with_capacity(beam_width.min(slot.len()));
    for state in slot.drain(..) {
        let last_word = match state.last_word_ref {
            Some((p, idx)) => spans_by_pos[p][idx].word.as_str(),
            None => seed_word,
        };
        let key = (last_word, state.typo_count);
        if best_by_key.contains_key(&key) {
            continue;
        }
        best_by_key.insert(key, kept.len());
        kept.push(state);
        if kept.len() >= beam_width {
            break;
        }
    }
    *slot = kept;
}

fn finalize_decode_result(
    lattice: &SyllableLattice,
    end_slot: &[BeamState],
    node_arena: &[ArenaNode],
    spans_by_pos: &[Vec<PhraseMatch>],
    mut whole_words: Vec<PhraseMatch>,
    mut prefix_word_matches: Vec<PhraseMatch>,
    options: &BeamDecodeOptions<'_>,
) -> DecodeResult {
    let mut sentences = Vec::with_capacity(end_slot.len().min(options.max_sentences));
    let mut seen_texts: Vec<String> = Vec::with_capacity(options.max_sentences);

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
        if seen_texts.contains(&text) {
            continue;
        }
        seen_texts.push(text.clone());

        let mut pinyin = String::new();
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
            .map(|&(p, idx)| spans_by_pos[p][idx].matched_key.clone())
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

    // 去重同一词条时，精确全拼与无容错变体必须优先保留，避免被低分容错/简拼路径挤掉
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

    let best_preedit = sentences
        .first()
        .filter(|s| s.typo_count == 0 && !s.has_initial && !s.pinyin.is_empty())
        .map(|s| s.pinyin.clone())
        .unwrap_or_else(|| lattice.best_formatted_preedit());

    DecodeResult {
        sentences,
        whole_words,
        prefix_heads,
        best_preedit,
    }
}

fn build_prefix_heads(
    sentences: &[DecodedSentence],
    prefix_word_matches: &[PhraseMatch],
    weights: &ScoringWeights,
) -> Vec<SentenceHeadSegment> {
    let mut heads = Vec::with_capacity(4);
    if let Some(best) = sentences.first() {
        if best.word_count >= 2 && best.first_word_end > 0 {
            heads.push(SentenceHeadSegment {
                text: best.first_word.clone(),
                pinyin: best.first_word_pinyin.clone(),
                consumed_len: best.first_word_end,
                score: best.score,
            });
        }
    }
    for pm in prefix_word_matches.iter().take(3) {
        if heads.iter().any(|h| h.text == pm.word) {
            continue;
        }
        heads.push(SentenceHeadSegment {
            text: pm.word.clone(),
            pinyin: pm.matched_key.clone(),
            consumed_len: pm.end,
            score: score_phrase_match(pm, weights),
        });
        if heads.len() >= 3 {
            break;
        }
    }
    heads
}

fn dedup_phrase_matches(matches: &mut Vec<PhraseMatch>) {
    let mut seen: Vec<String> = Vec::with_capacity(matches.len());
    matches.retain(|m| {
        if seen.contains(&m.word) {
            false
        } else {
            seen.push(m.word.clone());
            true
        }
    });
}

/// 计算单条词边的基础转移代价（纠错/混简拼/半成品惩罚与精确全拼奖励）
#[inline]
fn compute_edge_penalty(pm: &PhraseMatch, weights: &ScoringWeights) -> i64 {
    let mut penalty = (pm.edge_cost as i64).saturating_mul(weights.typo_cost_penalty as i64);
    if pm.typo_count > 0 {
        penalty = penalty.saturating_add((pm.typo_count as i64) * 4_500);
    }
    if pm.has_initial {
        penalty = penalty.saturating_add(12_000);
    }
    if pm.has_partial_tail {
        penalty = penalty.saturating_add((weights.partial_extension_penalty as i64) * 100 + 2_500);
    }
    penalty
}

/// 评估单条整词匹配的综合排序分（用于单位置候选排序与整词排序）
#[inline]
pub fn score_phrase_match(pm: &PhraseMatch, weights: &ScoringWeights) -> i64 {
    let char_len = pm.word.chars().count() as i64;
    let length_bonus = (char_len - 1).saturating_mul(weights.length_bonus_per_char);
    unigram_log_score(pm.freq) + length_bonus - compute_edge_penalty(pm, weights)
}

/// 将原始静态词频转换为与二元转移矩阵同量纲的对数似然得分
#[inline]
pub fn unigram_log_score(raw_score: u32) -> i64 {
    ((raw_score as f64 + 1.0).ln() * 1_200.0) as i64
}

/// 将束搜索路径累计对数似然安全映射为正整数候选分
#[inline]
pub fn to_positive_score(total_score: i64) -> u32 {
    if total_score <= 0 {
        1
    } else {
        (total_score.min(u32::MAX as i64 - 1_000_000) as u32).saturating_add(100)
    }
}
