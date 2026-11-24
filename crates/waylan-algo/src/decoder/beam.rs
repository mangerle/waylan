//! 按字节位置同步的统一束搜索解码器 (Positional Beam Decoder)
//!
//! # 设计原理
//! - **实现初衷**：在按字节索引的 `SyllableLattice` 上直接运行 `beam[0..=N]` 同步束搜索，
//!   使全拼、模糊音、四类容错（最大 `max_typos = 2`）、全拼混简拼与末尾半成品补全
//!   在同一束空间内直接竞争，消除旧版多路割裂与冗余路径枚举开销。
//! - **核心优势**：支持长句多处容错、自然产出精确字节偏移的句首词段 (`consumed_len`)，
//!   并在束内直接融合 `BigramLanguageModel` 与用户词对自适应提权。
//! - **代价与局限**：受束宽 `beam_width` 截断约束，极端同音长句可能剪枝低分尾部长句。

use super::beam_cache::{CacheSnapshot, IncrementalBeamCache};
use super::beam_english::{
    AlphaRunPinyinCache, append_english_lexicon_edges, append_fallback_edges,
};
use super::beam_expand::{ExpandContext, expand_position_states, init_beam_from_cache};
use super::beam_finalize::finalize_decode_result;
use super::beam_scoring::{filter_and_rank_matches, prune_beam_slot};
use super::lattice::SyllableLattice;
use super::matcher::{DatPhraseMatcher, PhraseMatch, RankedMatchParams};
use crate::custom_phrase::EnglishSuggestionManager;
use crate::table::LexiconTable;
use crate::user_adaptive::UserAdaptiveModel;

pub(super) use super::beam_types::{ArenaNode, BeamState};
pub use super::beam_types::{
    BeamDecodeOptions, DecodeResult, DecodedSentence, SentenceHeadSegment,
};

/// 束搜索运行期工作缓冲区
pub(super) struct BeamWorkspace {
    pub(super) beam: Vec<Vec<BeamState>>,
    pub(super) node_arena: Vec<ArenaNode>,
    pub(super) arena_len_before_pos: Vec<usize>,
    pub(super) spans_by_pos: Vec<Vec<PhraseMatch>>,
    pub(super) pos_max_reach: Vec<usize>,
    pub(super) loop_start: usize,
}

/// 解码起始辅助上下文（不可变参数聚合）
pub(super) struct InitContext<'a> {
    pub(super) lattice: &'a SyllableLattice,
    pub(super) table: &'a LexiconTable,
    pub(super) adaptive: Option<&'a UserAdaptiveModel>,
    pub(super) options: &'a BeamDecodeOptions<'a>,
    pub(super) start_pos: usize,
    pub(super) seed_word: &'a str,
}

/// 统一从首位置匹配集合中收集整词与句首词段候选
pub(super) fn collect_start_matches(
    matches: &[PhraseMatch],
    total_len: usize,
    whole_words: &mut Vec<PhraseMatch>,
    prefix_word_matches: &mut Vec<PhraseMatch>,
    include_whole: bool,
) {
    for m in matches {
        if include_whole && m.end == total_len {
            whole_words.push(m.clone());
        } else if m.end < total_len && m.syllable_count >= 2 && m.typo_count == 0 && !m.has_initial
        {
            prefix_word_matches.push(m.clone());
        }
    }
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

        let init_ctx = InitContext {
            lattice,
            table,
            adaptive,
            options,
            start_pos,
            seed_word,
        };

        let mut ws = if let Some(c) = cache.as_deref_mut()
            && let Some(freeze_pos) = c
                .compute_safe_freeze(lattice, options, config_sig)
                .filter(|&f| f > start_pos)
        {
            init_beam_from_cache(
                &init_ctx,
                c,
                freeze_pos,
                &mut whole_words,
                &mut prefix_word_matches,
            )
        } else {
            init_beam_cold(len, start_pos, seed_word_id)
        };

        advance_beam(
            &init_ctx,
            &mut ws,
            &mut whole_words,
            &mut prefix_word_matches,
            false,
        );

        let result = if ws.beam[len].is_empty() && len >= 3 {
            decode_connected_fallback(&init_ctx, seed_word_id, whole_words, prefix_word_matches)
        } else {
            finalize_decode_result(
                lattice,
                &ws.beam[len],
                &ws.node_arena,
                &ws.spans_by_pos,
                whole_words,
                prefix_word_matches,
                options,
            )
        };

        if let Some(c) = cache {
            c.update(
                &lattice.raw,
                config_sig,
                options.last_commit_context,
                CacheSnapshot {
                    beam_slots: ws.beam,
                    node_arena: ws.node_arena,
                    arena_len_before_pos: ws.arena_len_before_pos,
                    spans_by_pos: ws.spans_by_pos,
                    pos_max_reach: ws.pos_max_reach,
                },
            );
        }

        result
    }
}

/// 冷启动构造空束搜索工作区
fn init_beam_cold(len: usize, start_pos: usize, seed_word_id: Option<u32>) -> BeamWorkspace {
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

    BeamWorkspace {
        beam,
        node_arena,
        arena_len_before_pos,
        spans_by_pos,
        pos_max_reach,
        loop_start: start_pos,
    }
}

/// 断链兜底连通解码：放宽多处简拼与容错次数限制，并注入英文及单字符透传兜底边
fn decode_connected_fallback(
    ctx: &InitContext<'_>,
    seed_word_id: Option<u32>,
    whole_words: Vec<PhraseMatch>,
    prefix_word_matches: Vec<PhraseMatch>,
) -> DecodeResult {
    let len = ctx.lattice.len;
    let mut fb_ws = init_beam_cold(len, ctx.start_pos, seed_word_id);
    let mut unused_whole = Vec::new();
    let mut unused_prefix = Vec::new();
    advance_beam(ctx, &mut fb_ws, &mut unused_whole, &mut unused_prefix, true);
    finalize_decode_result(
        ctx.lattice,
        &fb_ws.beam[len],
        &fb_ws.node_arena,
        &fb_ws.spans_by_pos,
        whole_words,
        prefix_word_matches,
        ctx.options,
    )
}

/// 从 `loop_start` 到输入末尾逐位置推进束搜索扩展
fn advance_beam(
    ctx: &InitContext<'_>,
    ws: &mut BeamWorkspace,
    whole_words: &mut Vec<PhraseMatch>,
    prefix_word_matches: &mut Vec<PhraseMatch>,
    fallback_mode: bool,
) {
    let len = ctx.lattice.len;
    let beam_width = if fallback_mode {
        ctx.options.beam_width.min(16)
    } else {
        ctx.options.beam_width
    };
    let english_suggester = EnglishSuggestionManager::active_or_default();
    let run_cache = AlphaRunPinyinCache::new(&ctx.lattice.raw);
    for pos in ws.loop_start..len {
        ws.arena_len_before_pos[pos] = ws.node_arena.len();
        let next_non_sep = ctx.lattice.skip_separators(pos);
        if next_non_sep > pos {
            if !ws.beam[pos].is_empty() {
                let carried = std::mem::take(&mut ws.beam[pos]);
                ws.beam[next_non_sep].extend(carried);
                prune_beam_slot(
                    &mut ws.beam[next_non_sep],
                    beam_width,
                    &ws.spans_by_pos,
                    ctx.seed_word,
                );
            }
            continue;
        }
        if ws.beam[pos].is_empty() {
            continue;
        }

        prune_beam_slot(
            &mut ws.beam[pos],
            beam_width,
            &ws.spans_by_pos,
            ctx.seed_word,
        );
        let (mut matches, reach) =
            DatPhraseMatcher::match_ranked_from_with_reach(RankedMatchParams {
                lattice: ctx.lattice,
                start_pos: pos,
                table: ctx.table,
                options: &ctx.options.matcher_options,
                weights: &ctx.options.weights,
                keep_start_extras: pos == ctx.start_pos,
            });
        ws.pos_max_reach[pos] = reach;
        append_english_lexicon_edges(
            ctx.lattice,
            pos,
            &english_suggester,
            &run_cache,
            &mut matches,
        );
        if fallback_mode {
            append_fallback_edges(ctx.lattice, pos, &mut matches);
        }
        if matches.is_empty() {
            continue;
        }

        if pos == ctx.start_pos && !fallback_mode {
            collect_start_matches(&matches, len, whole_words, prefix_word_matches, true);
            filter_and_rank_matches(&mut matches, len, &ctx.options.weights);
        }

        ws.spans_by_pos[pos] = matches;

        let (left, right) = ws.beam.split_at_mut(pos + 1);
        let mut expand_ctx = ExpandContext {
            pos,
            beam_offset: pos + 1,
            seed_word: ctx.seed_word,
            spans_by_pos: &ws.spans_by_pos,
            table: ctx.table,
            adaptive: ctx.adaptive,
            options: ctx.options,
            fallback_mode,
            node_arena: &mut ws.node_arena,
            beam: right,
        };
        expand_position_states(&left[pos], &mut expand_ctx, None);
    }
}
