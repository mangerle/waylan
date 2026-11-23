//! 按字节位置同步的统一束搜索解码器 (Positional Beam Decoder)
//!
//! # 设计原理
//! - **实现初衷**：在按字节索引的 `SyllableLattice` 上直接运行 `beam[0..=N]` 同步束搜索，
//!   使全拼、模糊音、四类容错（最大 `max_typos = 2`）、全拼混简拼与末尾半成品补全
//!   在同一束空间内直接竞争，消除旧版多路割裂与冗余路径枚举开销。
//! - **核心优势**：支持长句多处容错、自然产出精确字节偏移的句首词段 (`consumed_len`)，
//!   并在束内直接融合 `BigramLanguageModel` 与用户词对自适应提权。
//! - **代价与局限**：受束宽 `beam_width` 截断约束，极端同音长句可能剪枝低分尾部长句。

use std::sync::Arc;

use super::beam_cache::{
    CacheSnapshot, IncrementalBeamCache, MAX_PHRASE_SPAN_BYTES, merge_crossing_matches,
};
use super::beam_finalize::finalize_decode_result;
use super::beam_scoring::{
    compute_edge_penalty, filter_and_rank_matches, prune_beam_slot, unigram_log_score,
};
use super::language_model::BigramLanguageModel;
use super::lattice::SyllableLattice;
use super::matcher::{DatPhraseMatcher, PhraseMatch};
use crate::custom_phrase::EnglishSuggestionManager;
use crate::syllables::{can_segment_into_syllables, is_valid_syllable};
use crate::table::LexiconTable;
use crate::user_adaptive::UserAdaptiveModel;

pub(super) use super::beam_types::{ArenaNode, BeamState};
pub use super::beam_types::{
    BeamDecodeOptions, DecodeResult, DecodedSentence, SentenceHeadSegment,
};

/// 束搜索运行期工作缓冲区
struct BeamWorkspace {
    beam: Vec<Vec<BeamState>>,
    node_arena: Vec<ArenaNode>,
    arena_len_before_pos: Vec<usize>,
    spans_by_pos: Vec<Vec<PhraseMatch>>,
    pos_max_reach: Vec<usize>,
    loop_start: usize,
}

/// 解码起始辅助上下文（不可变参数聚合）
struct InitContext<'a> {
    lattice: &'a SyllableLattice,
    table: &'a LexiconTable,
    adaptive: Option<&'a UserAdaptiveModel>,
    options: &'a BeamDecodeOptions<'a>,
    start_pos: usize,
    seed_word: &'a str,
}

/// 统一从首位置匹配集合中收集整词与句首词段候选
fn collect_start_matches(
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

        let freeze_opt = cache
            .as_deref()
            .and_then(|c| c.compute_safe_freeze(lattice, options, config_sig))
            .filter(|&f| f > start_pos);

        let init_ctx = InitContext {
            lattice,
            table,
            adaptive,
            options,
            start_pos,
            seed_word,
        };

        let mut ws = if let Some(freeze_pos) = freeze_opt {
            let c = cache.as_deref_mut().expect("cache checked above");
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

/// 从增量缓存冻结边界恢复束搜索工作区，并补充跨边界匹配边
fn init_beam_from_cache(
    ctx: &InitContext<'_>,
    c: &mut IncrementalBeamCache,
    freeze_pos: usize,
    whole_words: &mut Vec<PhraseMatch>,
    prefix_word_matches: &mut Vec<PhraseMatch>,
) -> BeamWorkspace {
    let len = ctx.lattice.len;
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
    arena_len_before_pos[..freeze_pos].copy_from_slice(&c.arena_len_before_pos[..freeze_pos]);

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

    let bridge_start = freeze_pos.saturating_sub(MAX_PHRASE_SPAN_BYTES);
    for pos in bridge_start..freeze_pos {
        if beam[pos].is_empty() || pos_max_reach[pos] < freeze_pos {
            if pos == ctx.start_pos {
                collect_start_matches(
                    &spans_by_pos[ctx.start_pos],
                    len,
                    whole_words,
                    prefix_word_matches,
                    false,
                );
            }
            continue;
        }
        let (mut fresh, reach) = DatPhraseMatcher::match_ranked_from_with_reach(
            ctx.lattice,
            pos,
            ctx.table,
            &ctx.options.matcher_options,
            &ctx.options.weights,
            pos == ctx.start_pos,
        );
        pos_max_reach[pos] = reach;
        if pos == ctx.start_pos {
            collect_start_matches(&fresh, len, whole_words, prefix_word_matches, true);
            filter_and_rank_matches(&mut fresh, len, &ctx.options.weights);
        }
        merge_crossing_matches(&mut spans_by_pos[pos], fresh, freeze_pos);

        let (left, right) = beam.split_at_mut(pos + 1);
        let mut expand_ctx = ExpandContext {
            pos,
            beam_offset: pos + 1,
            seed_word: ctx.seed_word,
            spans_by_pos: &spans_by_pos,
            table: ctx.table,
            adaptive: ctx.adaptive,
            options: ctx.options,
            fallback_mode: false,
            node_arena: &mut node_arena,
            beam: right,
        };
        expand_position_states(&left[pos], &mut expand_ctx, Some(freeze_pos));
    }

    if ctx.start_pos < bridge_start {
        collect_start_matches(
            &spans_by_pos[ctx.start_pos],
            len,
            whole_words,
            prefix_word_matches,
            false,
        );
    }

    BeamWorkspace {
        beam,
        node_arena,
        arena_len_before_pos,
        spans_by_pos,
        pos_max_reach,
        loop_start: freeze_pos,
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

/// 非合法汉语拼音的高频技术英文短词集合（免切中英混输场景给予优先权重）
const HIGH_FREQ_TECH_WORDS: &[&str] = &[
    "api", "bug", "cli", "cpu", "css", "dns", "docker", "git", "gpu", "html", "ide", "json", "jwt",
    "linux", "python", "rpc", "rust", "sdk", "sql", "ssh", "tcp", "tls", "toml", "udp", "url",
    "vim", "yaml",
];

/// 判断英文词条是否为高频技术短词（忽略大小写）
#[inline]
fn is_high_freq_tech_word(slice: &str) -> bool {
    HIGH_FREQ_TECH_WORDS
        .iter()
        .any(|w| w.eq_ignore_ascii_case(slice))
}

/// 判断非技术短词的英文候选是否会误截胡可被中文完整切分的全拼区间
///
/// # 判定规则
/// 1. 英文词自身区间 `[pos, end)` 可完整切分为合法全拼音节（如 `xigua`、`keyide`）时拒绝，
///    不再受字节长度上限约束，覆盖三音节及以上的长区间；
/// 2. 英文词未抵达连续字母段末尾（非尾部对齐），且 `[pos, run_end)` 整段可被全拼切分时，
///    说明该英文词只是中文长串中的非对齐碎片（如 `xiguapi` 中的前缀碎片），同样拒绝。
///
/// # 设计取舍
/// 仅依赖音节可切分性判断，不查词库，开销为两次 O(n) 动态规划；
/// 代价是极少数恰好可被全拼切分的真实英文词（如 `panda`）需通过技术词表或 `aux.dict`
/// 之外的途径提权，不会再由本函数注入。
fn is_chinese_pinyin_hijack(raw: &str, pos: usize, end: usize) -> bool {
    if can_segment_into_syllables(&raw[pos..end]) {
        return true;
    }
    let run_end = raw[pos..]
        .bytes()
        .position(|b| !b.is_ascii_alphabetic())
        .map_or(raw.len(), |off| pos + off);
    end < run_end && can_segment_unbounded(&raw[pos..run_end])
}

/// 不限长度的全拼可切分判定（`can_segment_into_syllables` 受 32 字节上限，无法覆盖整句输入）
fn can_segment_unbounded(s: &str) -> bool {
    if s.is_empty() || !s.is_ascii() {
        return false;
    }
    let n = s.len();
    let mut reachable = vec![false; n + 1];
    reachable[0] = true;
    for i in 0..n {
        if !reachable[i] {
            continue;
        }
        for len in 1..=(n - i).min(6) {
            if is_valid_syllable(&s[i..i + len]) {
                reachable[i + len] = true;
            }
        }
    }
    reachable[n]
}

/// 判断技术词候选是否只是整段纯拼音串中的一部分（如 `xiguapi` 中的 `api`、`meimeide` 中的 `ide`）
///
/// # 判定规则
/// 取 `[pos, end)` 所在的连续字母段，若该段本身不等于技术词，且整段可被全拼完整切分，
/// 说明存在完整的中文读法，技术词只是跨音节边界的偶然子串，应拒绝注入。
///
/// # 设计取舍
/// 通用规则，不依赖具体词表；代价是 `woyongapiwenti` 这类整段恰好也可读成全拼的中英混输，
/// 技术词会被拒绝，需用户以大写或分隔符显式输入英文。
fn is_tech_word_inside_pinyin_run(raw: &str, pos: usize, end: usize) -> bool {
    let run_start = raw[..pos]
        .bytes()
        .rposition(|b| !b.is_ascii_alphabetic())
        .map_or(0, |i| i + 1);
    let run_end = raw[end..]
        .bytes()
        .position(|b| !b.is_ascii_alphabetic())
        .map_or(raw.len(), |off| end + off);
    if run_start == pos && run_end == end {
        return false;
    }
    can_segment_unbounded(&raw[run_start..run_end])
}

/// 在词图当前位置注入高频英文词库边，实现免切中英混输（如 woxiangqugoogle -> 我想去google）
///
/// # 防截胡与提权策略
/// 1. 若同跨度已命中高频精确中文词（如 `pinyin` -> “拼音”），禁止英文边截胡中文；
/// 2. 对非合法拼音的高频技术短词（如 `bug`、`api`、`git`）给予零边惩罚与高频提权，避免被中文碎词吞没。
fn append_english_lexicon_edges(
    lattice: &SyllableLattice,
    pos: usize,
    eng: &EnglishSuggestionManager,
    matches: &mut Vec<PhraseMatch>,
) {
    let hits = eng.match_exact_prefixes(&lattice.raw[pos..], 3, 24);
    let mut added_tech_end: Option<usize> = None;
    for (orig_word, eng_freq, byte_len) in hits {
        let end = pos + byte_len;
        let slice = &lattice.raw[pos..end];
        let is_tech = is_high_freq_tech_word(slice);
        if is_valid_syllable(slice)
            || (!is_tech && is_chinese_pinyin_hijack(&lattice.raw, pos, end))
            || (is_tech && is_tech_word_inside_pinyin_run(&lattice.raw, pos, end))
        {
            continue;
        }
        // 若同跨度已存在无容错、无简拼的高频精确中文词（如 pinyin -> 拼音），禁止英文边截胡
        let blocked_by_chinese = matches.iter().any(|m| {
            m.end == end && m.is_exact_all && m.typo_count == 0 && !m.has_initial && m.freq >= 500
        });
        if blocked_by_chinese {
            continue;
        }
        if is_tech {
            added_tech_end = Some(end);
        }
        matches.push(PhraseMatch {
            start: pos,
            end,
            word: orig_word.to_string(),
            word_id: None,
            freq: if is_tech {
                eng_freq.max(80_000)
            } else {
                eng_freq.max(10_000)
            },
            matched_key: Arc::from(slice),
            syllable_count: 1,
            typo_count: 0,
            edge_cost: if is_tech { 0 } else { 12 },
            has_initial: false,
            has_partial_tail: false,
            is_exact_all: false,
        });
    }

    // 保底兜底：即使未加载外部 aux.dict，也确保内置高频技术短词（如 bug/api/git）可参与混输
    for &tech in HIGH_FREQ_TECH_WORDS {
        let tlen = tech.len();
        if pos + tlen > lattice.len {
            continue;
        }
        let end = pos + tlen;
        if added_tech_end == Some(end) {
            continue;
        }
        let slice = &lattice.raw[pos..end];
        if !slice.eq_ignore_ascii_case(tech)
            || is_tech_word_inside_pinyin_run(&lattice.raw, pos, end)
        {
            continue;
        }
        if matches
            .iter()
            .any(|m| m.end == end && m.word.eq_ignore_ascii_case(tech))
        {
            continue;
        }
        matches.push(PhraseMatch {
            start: pos,
            end,
            word: tech.to_string(),
            word_id: None,
            freq: 80_000,
            matched_key: Arc::from(slice),
            syllable_count: 1,
            typo_count: 0,
            edge_cost: 0,
            has_initial: false,
            has_partial_tail: false,
            is_exact_all: false,
        });
    }
}

/// 在兜底连通解码模式下追加英文单词兜底边与单字符防断链边
fn append_fallback_edges(lattice: &SyllableLattice, pos: usize, matches: &mut Vec<PhraseMatch>) {
    let raw = lattice.raw.as_bytes();
    let len = lattice.len;
    let mut seg_end = pos;
    while seg_end < len && raw[seg_end].is_ascii_alphabetic() {
        seg_end += 1;
    }
    let seg_len = seg_end.saturating_sub(pos);
    if (2..=12).contains(&seg_len) && (seg_end == len || raw[seg_end] == b'\'') {
        let seg = &lattice.raw[pos..seg_end];
        if !is_valid_syllable(seg) {
            matches.push(PhraseMatch {
                start: pos,
                end: seg_end,
                word: seg.to_string(),
                word_id: None,
                freq: 1,
                matched_key: Arc::from(seg),
                syllable_count: 1,
                typo_count: 1,
                edge_cost: 220,
                has_initial: false,
                has_partial_tail: false,
                is_exact_all: false,
            });
        }
    }

    if !matches.iter().any(|m| m.end > pos) {
        let char_len = lattice.raw[pos..].chars().next().map_or(1, char::len_utf8);
        let next_pos = (pos + char_len).min(len);
        let ch_str = &lattice.raw[pos..next_pos];
        matches.push(PhraseMatch {
            start: pos,
            end: next_pos,
            word: ch_str.to_string(),
            word_id: None,
            freq: 1,
            matched_key: Arc::from(ch_str),
            syllable_count: 1,
            typo_count: 1,
            edge_cost: 350,
            has_initial: false,
            has_partial_tail: false,
            is_exact_all: false,
        });
    }
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
        let (mut matches, reach) = DatPhraseMatcher::match_ranked_from_with_reach(
            ctx.lattice,
            pos,
            ctx.table,
            &ctx.options.matcher_options,
            &ctx.options.weights,
            pos == ctx.start_pos,
        );
        ws.pos_max_reach[pos] = reach;
        append_english_lexicon_edges(ctx.lattice, pos, &english_suggester, &mut matches);
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

struct ExpandContext<'a> {
    pos: usize,
    beam_offset: usize,
    seed_word: &'a str,
    spans_by_pos: &'a [Vec<PhraseMatch>],
    table: &'a LexiconTable,
    adaptive: Option<&'a UserAdaptiveModel>,
    options: &'a BeamDecodeOptions<'a>,
    fallback_mode: bool,
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
    let max_typos = if ctx.fallback_mode {
        u8::MAX
    } else {
        ctx.options.max_typos
    };
    for state in cur_states {
        let last_word = match state.last_word_ref {
            Some((p, idx)) => ctx.spans_by_pos[p][idx].word.as_str(),
            None => ctx.seed_word,
        };

        for (idx, pm) in pos_matches.iter().enumerate() {
            if let Some(freeze) = min_next_pos
                && pm.end <= freeze
            {
                continue;
            }
            if !ctx.fallback_mode && state.has_initial && pm.has_initial && state.word_count > 0 {
                continue;
            }
            let next_typo = state.typo_count.saturating_add(pm.typo_count);
            if next_typo > max_typos {
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

            ctx.beam[next_pos - ctx.beam_offset].push(BeamState {
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

#[cfg(test)]
mod hijack_tests {
    use super::{is_chinese_pinyin_hijack, is_tech_word_inside_pinyin_run};

    #[test]
    fn 可切分长区间应判定为截胡() {
        assert!(is_chinese_pinyin_hijack("xiguapi", 0, 5));
        assert!(is_chinese_pinyin_hijack("keyide", 0, 6));
        assert!(is_chinese_pinyin_hijack("mainage", 0, 4));
    }

    #[test]
    fn 非对齐碎片但整段可切分应判定为截胡() {
        assert!(is_chinese_pinyin_hijack("xiguapi", 0, 4));
    }

    #[test]
    fn 真实英文词不应被误拦() {
        assert!(!is_chinese_pinyin_hijack("google", 0, 6));
        assert!(!is_chinese_pinyin_hijack("googlewoai", 0, 6));
    }

    #[test]
    fn 技术词位于纯拼音长串内部应被拒绝() {
        let s = "xiguapilifangdongxileba";
        assert!(is_tech_word_inside_pinyin_run(s, 4, 7));
        let s = "wodasanyaxianzaihaoduomeimeidedifangya";
        assert!(is_tech_word_inside_pinyin_run(s, 27, 30));
    }

    #[test]
    fn 技术词独立或所在串无法读成拼音应保留() {
        assert!(!is_tech_word_inside_pinyin_run("bug", 0, 3));
        assert!(!is_tech_word_inside_pinyin_run("wodebugyouduo", 4, 7));
        assert!(!is_tech_word_inside_pinyin_run("woxihuanrust", 8, 12));
    }
}
