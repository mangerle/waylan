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

#[derive(Clone)]
struct BeamState {
    score: i64,
    typo_count: u8,
    has_initial: bool,
    has_partial_tail: bool,
    is_exact_all: bool,
    last_word: String,
    last_word_id: Option<u32>,
    words: Vec<String>,
    pinyin_parts: Vec<String>,
    first_word_end: Option<usize>,
}

/// 按字节位置同步的统一束搜索解码器
pub struct PositionalBeamDecoder;

impl PositionalBeamDecoder {
    /// 在统一音节格上执行端到端束搜索解码
    pub fn decode(
        lattice: &SyllableLattice,
        table: &LexiconTable,
        adaptive: Option<&UserAdaptiveModel>,
        options: &BeamDecodeOptions<'_>,
    ) -> DecodeResult {
        let len = lattice.len;
        if len == 0 {
            return DecodeResult::default();
        }

        let mut beam: Vec<Vec<BeamState>> = vec![Vec::new(); len + 1];
        let seed_word = options.last_commit_context.unwrap_or("").to_string();
        let seed_word_id = (!seed_word.is_empty())
            .then(|| table.get_word_id(&seed_word))
            .flatten();

        let start_pos = lattice.skip_separators(0);
        if start_pos >= len {
            return DecodeResult::default();
        }
        beam[start_pos].push(BeamState {
            score: 0,
            typo_count: 0,
            has_initial: false,
            has_partial_tail: false,
            is_exact_all: true,
            last_word: seed_word,
            last_word_id: seed_word_id,
            words: Vec::new(),
            pinyin_parts: Vec::new(),
            first_word_end: None,
        });

        let mut whole_words: Vec<PhraseMatch> = Vec::new();
        let mut prefix_word_matches: Vec<PhraseMatch> = Vec::new();

        for pos in start_pos..len {
            let next_non_sep = lattice.skip_separators(pos);
            if next_non_sep > pos {
                if !beam[pos].is_empty() {
                    let carried = std::mem::take(&mut beam[pos]);
                    beam[next_non_sep].extend(carried);
                    prune_beam_slot(&mut beam[next_non_sep], options.beam_width);
                }
                continue;
            }
            if beam[pos].is_empty() {
                continue;
            }

            prune_beam_slot(&mut beam[pos], options.beam_width);
            let matches =
                DatPhraseMatcher::match_from(lattice, pos, table, &options.matcher_options);
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

            let cur_states = std::mem::take(&mut beam[pos]);
            expand_position_states(&cur_states, &matches, table, adaptive, options, &mut beam);
            beam[pos] = cur_states;
        }

        finalize_decode_result(lattice, beam, whole_words, prefix_word_matches, options)
    }
}

fn expand_position_states(
    cur_states: &[BeamState],
    matches: &[PhraseMatch],
    table: &LexiconTable,
    adaptive: Option<&UserAdaptiveModel>,
    options: &BeamDecodeOptions<'_>,
    beam: &mut [Vec<BeamState>],
) {
    let mut ranked_matches: Vec<(i64, &PhraseMatch)> = matches
        .iter()
        .map(|m| (score_phrase_match(m, &options.weights), m))
        .collect();
    ranked_matches.sort_by_key(|a| std::cmp::Reverse(a.0));
    ranked_matches.truncate(48);

    for state in cur_states {
        for &(_, pm) in &ranked_matches {
            if state.has_initial && pm.has_initial && !state.words.is_empty() {
                continue;
            }
            let next_typo = state.typo_count.saturating_add(pm.typo_count);
            if next_typo > options.max_typos {
                continue;
            }
            let next_pos = pm.end;
            let char_len = pm.word.chars().count() as i64;
            let curr_unigram = unigram_log_score(pm.freq);
            let lm_delta = BigramLanguageModel::transition_score_with_weights(
                (&state.last_word, state.last_word_id),
                (&pm.word, pm.word_id, char_len),
                curr_unigram,
                (table, &options.weights),
            );
            let pair_boost = adaptive
                .filter(|_| !state.last_word.is_empty())
                .map(|a| a.pair_boost_score(&state.last_word, &pm.word, options.eval_ts))
                .unwrap_or(0);
            let edge_penalty = compute_edge_penalty(pm, &options.weights);

            let mut next_words = state.words.clone();
            next_words.push(pm.word.clone());
            let mut next_pinyins = state.pinyin_parts.clone();
            next_pinyins.push(pm.matched_key.clone());

            beam[next_pos].push(BeamState {
                score: state.score + lm_delta + pair_boost - edge_penalty,
                typo_count: next_typo,
                has_initial: state.has_initial || pm.has_initial,
                has_partial_tail: state.has_partial_tail || pm.has_partial_tail,
                is_exact_all: state.is_exact_all && pm.is_exact_all,
                last_word: pm.word.clone(),
                last_word_id: pm.word_id,
                words: next_words,
                pinyin_parts: next_pinyins,
                first_word_end: state.first_word_end.or(Some(pm.end)),
            });
        }
    }
}

fn prune_beam_slot(slot: &mut Vec<BeamState>, beam_width: usize) {
    if slot.len() <= 1 {
        return;
    }
    slot.sort_by_key(|a| std::cmp::Reverse(a.score));

    let mut best_by_key: HashMap<(String, u8), usize> = HashMap::with_capacity(slot.len().min(64));
    let mut kept = Vec::with_capacity(beam_width.min(slot.len()));
    for state in slot.drain(..) {
        let key = (state.last_word.clone(), state.typo_count);
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
    mut beam: Vec<Vec<BeamState>>,
    mut whole_words: Vec<PhraseMatch>,
    mut prefix_word_matches: Vec<PhraseMatch>,
    options: &BeamDecodeOptions<'_>,
) -> DecodeResult {
    let end_slot = beam.pop().unwrap_or_default();
    let mut sentences = Vec::with_capacity(end_slot.len().min(options.max_sentences));
    let mut seen_texts: Vec<String> = Vec::with_capacity(options.max_sentences);

    let mut sorted_end = end_slot;
    sorted_end.sort_by_key(|a| std::cmp::Reverse(a.score));

    for st in sorted_end {
        let text: String = st.words.concat();
        if seen_texts.contains(&text) {
            continue;
        }
        seen_texts.push(text.clone());
        sentences.push(DecodedSentence {
            text,
            pinyin: st.pinyin_parts.join("'"),
            score: st.score,
            word_count: st.words.len(),
            typo_count: st.typo_count,
            has_initial: st.has_initial,
            has_partial_tail: st.has_partial_tail,
            is_exact_all: st.is_exact_all,
            first_word: st.words.first().cloned().unwrap_or_default(),
            first_word_pinyin: st.pinyin_parts.first().cloned().unwrap_or_default(),
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
        penalty = penalty.saturating_add(8_000);
    }
    if pm.has_partial_tail {
        penalty = penalty.saturating_add((weights.partial_extension_penalty as i64) * 100);
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
