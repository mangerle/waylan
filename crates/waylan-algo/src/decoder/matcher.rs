//! 基于双数组 Trie (DAT) 的音节格同步词组匹配器
//!
//! # 设计原理
//! - **实现初衷**：在统一音节格的任意字节起点出发，沿音节边同步推进 `DatCursor`，
//!   直接匹配 1..=6 音节的词典词条。遇到混简拼声母边 (`Initial`) 或末尾半成品边 (`PartialTail`)
//!   时直接在当前 DAT 节点展开真实存在的音节分支，彻底消除旧版 `acronym_entries` 的 Top-K 截断问题。
//! - **核心优势**：零多余堆分配，无效前缀在 Trie 层立即剪枝；全量检索与测试均统一运行在零拷贝 DAT 视图上。
//! - **代价与局限**：单次词匹配最多跨越 6 个音节与 4 个混简拼步进，以防止极端输入组合扩散。

use std::sync::Arc;

use super::alias::calibrate_habitual_alias_score;
use super::beam_scoring::{compute_raw_match_score, RawScoreParams};
use super::lattice::{EdgeKind, SyllableEdge, SyllableLattice};
use super::weights::ScoringWeights;
use crate::dat::{DatCursor, DatReader};
use crate::pipeline::is_rare_ext_char;
use crate::table::{CompactMmapLexicon, LexiconTable};

/// 词组匹配结果边（供束搜索解码器消费）
#[derive(Debug, Clone)]
pub struct PhraseMatch {
    /// 原始输入中的起始字节偏移
    pub start: usize,
    /// 原始输入中的结束字节偏移
    pub end: usize,
    /// 命中的中文词条文本
    pub word: String,
    /// 词条在紧凑词典中的唯一 ID（用于 O(1) Bigram 查表）
    pub word_id: Option<u32>,
    /// 词条词频（经惯用音/长度校准前或后的词频得分）
    pub freq: u32,
    /// 命中的规范拼音键（如 `"wan'quan'ke'yi"`），同键多词条共享同一底层切片
    pub matched_key: Arc<str>,
    /// 覆盖音节数
    pub syllable_count: usize,
    /// 消耗的纠错次数
    pub typo_count: u8,
    /// 累计图边惩罚代价
    pub edge_cost: u32,
    /// 是否包含混简拼声母扩展
    pub has_initial: bool,
    /// 是否包含末尾半成品前缀扩展
    pub has_partial_tail: bool,
    /// 是否整条词边完全由精确全拼构成
    pub is_exact_all: bool,
}

/// 词组匹配配置选项
#[derive(Debug, Clone, Copy)]
pub struct MatcherOptions {
    /// 单个词条最多跨越的音节数（默认 6）
    pub max_syllables: usize,
    /// 单个词条最多允许消耗的纠错次数（默认 2）
    pub max_typos: u8,
    /// 每个命中的拼音键最多拉取的同音候选词条数（默认 12）
    pub top_k_per_key: usize,
    /// 单条词路径最多允许包含的混简拼声母步数（默认 4）
    pub max_initial_steps: usize,
    /// 是否过滤 CJK Ext-B 及以上生僻单字
    pub filter_rare_ext_chars: bool,
}

impl Default for MatcherOptions {
    fn default() -> Self {
        Self {
            max_syllables: 6,
            max_typos: 2,
            top_k_per_key: 12,
            max_initial_steps: 4,
            filter_rare_ext_chars: true,
        }
    }
}

/// 基于 DAT 与内存词表的统一词组匹配器
pub struct DatPhraseMatcher;

impl DatPhraseMatcher {
    /// 从音节格的指定字节位置 `start_pos` 出发，匹配所有可达的 1..=N 音节词条
    pub fn match_from(
        lattice: &SyllableLattice,
        start_pos: usize,
        table: &LexiconTable,
        options: &MatcherOptions,
    ) -> Vec<PhraseMatch> {
        Self::match_from_with_reach(lattice, start_pos, table, options).0
    }

    /// 从音节格指定位置匹配词条，同时返回 DAT 前缀探索触及的最远字节位置
    pub fn match_from_with_reach(
        lattice: &SyllableLattice,
        start_pos: usize,
        table: &LexiconTable,
        options: &MatcherOptions,
    ) -> (Vec<PhraseMatch>, usize) {
        let Some(mmap) = &table.compact_mmap else {
            return (Vec::new(), start_pos);
        };
        let (raw, max_reach) = Self::collect_raw_matches(lattice, start_pos, mmap, options);
        let results = materialize_raw_matches(mmap, start_pos, raw);
        (results, max_reach)
    }

    /// 束搜索热路径专用：先对轻量索引结果执行生僻单字过滤、打分与 Top-48 截断，再惰性物化 `word: String`
    ///
    /// 当 `keep_start_extras` 为 `true` 时，额外保留覆盖全长整词与未容错无简拼的句首词段候选，
    /// 供起始位置收集 `whole_words` 与 `prefix_word_matches`。
    pub(super) fn match_ranked_from_with_reach(
        lattice: &SyllableLattice,
        start_pos: usize,
        table: &LexiconTable,
        options: &MatcherOptions,
        weights: &ScoringWeights,
        keep_start_extras: bool,
    ) -> (Vec<PhraseMatch>, usize) {
        let Some(mmap) = &table.compact_mmap else {
            return (Vec::new(), start_pos);
        };
        let (mut raw, max_reach) = Self::collect_raw_matches(lattice, start_pos, mmap, options);
        if raw.is_empty() {
            return (Vec::new(), max_reach);
        }

        let target_len = lattice.len;
        let has_common_at_end: Vec<usize> =
            raw.iter().filter(|o| o.freq >= 10).map(|o| o.end).collect();

        raw.retain(|m| {
            let is_rare_single = m.syllable_count == 1
                && m.freq < 10
                && m.char_len == 1
                && (start_pos > 0 || m.end < target_len);
            if !is_rare_single {
                return true;
            }
            !has_common_at_end.contains(&m.end)
        });

        raw.sort_by_key(|m| std::cmp::Reverse(score_raw_match(m, weights)));
        if keep_start_extras {
            let mut kept_top = 0usize;
            raw.retain(|m| {
                let is_start_extra = m.end == target_len
                    || (m.end < target_len
                        && m.syllable_count >= 2
                        && m.typo_count == 0
                        && !m.has_initial);
                if kept_top < 48 {
                    kept_top += 1;
                    true
                } else {
                    is_start_extra
                }
            });
        } else {
            raw.truncate(48);
        }

        let results = materialize_raw_matches(mmap, start_pos, raw);
        (results, max_reach)
    }

    fn collect_raw_matches(
        lattice: &SyllableLattice,
        start_pos: usize,
        mmap: &CompactMmapLexicon,
        options: &MatcherOptions,
    ) -> (Vec<RawPhraseMatch>, usize) {
        let mut results = Vec::new();
        if start_pos >= lattice.len {
            return (results, start_pos);
        }

        let reader = mmap.get_dat_reader();
        let mut ctx = DatSearchCtx {
            lattice,
            mmap,
            reader,
            options,
            max_reach: start_pos,
            key_buf: String::with_capacity(64),
            results: &mut results,
        };
        let state = DatWalkState {
            pos: start_pos,
            cursor: reader.root_cursor(),
            syl_count: 0,
            initial_steps: 0,
            typo_count: 0,
            edge_cost: 0,
            has_initial: false,
            has_partial_tail: false,
            is_exact_all: true,
            has_habitual_alias: false,
        };
        dfs_dat_match(&mut ctx, state);
        let max_reach = ctx.max_reach;
        (results, max_reach)
    }
}

/// 延迟物化的轻量词组匹配中间条目（不含 `word: String`，避免被剪枝候选产生堆分配）
struct RawPhraseMatch {
    end: usize,
    word_id: u32,
    char_len: u8,
    freq: u32,
    matched_key: Arc<str>,
    syllable_count: usize,
    typo_count: u8,
    edge_cost: u32,
    has_initial: bool,
    has_partial_tail: bool,
    is_exact_all: bool,
}

#[inline]
fn score_raw_match(m: &RawPhraseMatch, weights: &ScoringWeights) -> i64 {
    compute_raw_match_score(
        RawScoreParams {
            freq: m.freq,
            char_len: m.char_len as usize,
            edge_cost: m.edge_cost,
            typo_count: m.typo_count,
            has_initial: m.has_initial,
            has_partial_tail: m.has_partial_tail,
        },
        weights,
    )
}

fn materialize_raw_matches(
    mmap: &CompactMmapLexicon,
    start_pos: usize,
    raw: Vec<RawPhraseMatch>,
) -> Vec<PhraseMatch> {
    let mut out = Vec::with_capacity(raw.len());
    for r in raw {
        if let Some(word) = mmap.get_word(r.word_id) {
            out.push(PhraseMatch {
                start: start_pos,
                end: r.end,
                word: word.to_string(),
                word_id: Some(r.word_id),
                freq: r.freq,
                matched_key: r.matched_key,
                syllable_count: r.syllable_count,
                typo_count: r.typo_count,
                edge_cost: r.edge_cost,
                has_initial: r.has_initial,
                has_partial_tail: r.has_partial_tail,
                is_exact_all: r.is_exact_all,
            });
        }
    }
    out
}

struct DatSearchCtx<'a> {
    lattice: &'a SyllableLattice,
    mmap: &'a CompactMmapLexicon,
    reader: DatReader<'a>,
    options: &'a MatcherOptions,
    max_reach: usize,
    /// DFS 共享可回溯拼音键拼接缓冲，消除每分支 Vec<String> 克隆与 join 堆分配
    key_buf: String,
    results: &'a mut Vec<RawPhraseMatch>,
}

#[derive(Clone, Copy)]
struct DatWalkState {
    pos: usize,
    cursor: DatCursor,
    syl_count: usize,
    initial_steps: usize,
    typo_count: u8,
    edge_cost: u32,
    has_initial: bool,
    has_partial_tail: bool,
    is_exact_all: bool,
    has_habitual_alias: bool,
}

fn dfs_dat_match(ctx: &mut DatSearchCtx<'_>, state: DatWalkState) {
    if state.pos >= ctx.lattice.len || state.syl_count >= ctx.options.max_syllables {
        return;
    }
    let edges = &ctx.lattice.edges_from[state.pos];
    for edge in edges {
        if state.typo_count.saturating_add(edge.typo_count) > ctx.options.max_typos {
            continue;
        }
        if state.syl_count + edge.syllables.len() > ctx.options.max_syllables {
            continue;
        }
        match edge.kind {
            EdgeKind::Initial => handle_dat_initial_edge(ctx, &state, edge),
            EdgeKind::PartialTail => handle_dat_partial_edge(ctx, &state, edge),
            _ => handle_dat_standard_edge(ctx, &state, edge),
        }
    }
}

fn handle_dat_standard_edge(ctx: &mut DatSearchCtx<'_>, state: &DatWalkState, edge: &SyllableEdge) {
    let mut cursor = state.cursor;
    let saved_len = ctx.key_buf.len();
    for (idx, syl) in edge.syllables.iter().enumerate() {
        if state.syl_count + idx > 0 {
            let Some(sep_c) = cursor.step_byte(&ctx.reader, b'\'') else {
                ctx.key_buf.truncate(saved_len);
                return;
            };
            cursor = sep_c;
            ctx.key_buf.push('\'');
        }
        let Some(syl_c) = cursor.step_str(&ctx.reader, syl) else {
            ctx.key_buf.truncate(saved_len);
            return;
        };
        cursor = syl_c;
        ctx.key_buf.push_str(syl);
    }

    ctx.max_reach = ctx.max_reach.max(edge.end);
    let next_syl_count = state.syl_count + edge.syllables.len();
    let next_typo = state.typo_count.saturating_add(edge.typo_count);
    let next_cost = state.edge_cost.saturating_add(edge.cost);
    let is_exact = state.is_exact_all && edge.kind == EdgeKind::Exact;
    let has_alias = state.has_habitual_alias || edge.kind == EdgeKind::RuleNormalized;

    if let Some(key_idx) = cursor.terminal_value(&ctx.reader) {
        emit_dat_candidates(
            ctx,
            key_idx as usize,
            MatchMeta {
                end: edge.end,
                syllable_count: next_syl_count,
                typo_count: next_typo,
                edge_cost: next_cost,
                has_initial: state.has_initial,
                has_partial_tail: state.has_partial_tail,
                is_exact_all: is_exact,
                has_habitual_alias: has_alias,
            },
        );
    }

    let next_pos = ctx.lattice.skip_separators(edge.end);
    if next_pos < ctx.lattice.len && next_syl_count < ctx.options.max_syllables {
        dfs_dat_match(
            ctx,
            DatWalkState {
                pos: next_pos,
                cursor,
                syl_count: next_syl_count,
                initial_steps: state.initial_steps,
                typo_count: next_typo,
                edge_cost: next_cost,
                has_initial: state.has_initial,
                has_partial_tail: state.has_partial_tail,
                is_exact_all: is_exact,
                has_habitual_alias: has_alias,
            },
        );
    }
    ctx.key_buf.truncate(saved_len);
}

fn handle_dat_initial_edge(ctx: &mut DatSearchCtx<'_>, state: &DatWalkState, edge: &SyllableEdge) {
    if state.initial_steps >= ctx.options.max_initial_steps {
        return;
    }
    let Some(initial) = edge.syllables.first() else {
        return;
    };
    let mut cursor = state.cursor;
    if state.syl_count > 0 {
        let Some(sep_c) = cursor.step_byte(&ctx.reader, b'\'') else {
            return;
        };
        cursor = sep_c;
    }
    let mut branches = Vec::with_capacity(32);
    cursor.collect_syllable_branches(&ctx.reader, initial, 32, &mut branches);
    if !branches.is_empty() {
        ctx.max_reach = ctx.max_reach.max(edge.end);
    }

    let next_pos = ctx.lattice.skip_separators(edge.end);
    let next_cost = state.edge_cost.saturating_add(edge.cost);
    let next_syl_count = state.syl_count + 1;
    let saved_len = ctx.key_buf.len();

    for (syl, next_cursor) in branches {
        if state.syl_count > 0 {
            ctx.key_buf.push('\'');
        }
        ctx.key_buf.push_str(&syl);
        if next_syl_count >= 2 {
            if let Some(key_idx) = next_cursor.terminal_value(&ctx.reader) {
                emit_dat_candidates(
                    ctx,
                    key_idx as usize,
                    MatchMeta {
                        end: edge.end,
                        syllable_count: next_syl_count,
                        typo_count: state.typo_count,
                        edge_cost: next_cost,
                        has_initial: true,
                        has_partial_tail: state.has_partial_tail,
                        is_exact_all: false,
                        has_habitual_alias: state.has_habitual_alias,
                    },
                );
            }
        }
        if next_pos < ctx.lattice.len && next_syl_count < ctx.options.max_syllables {
            dfs_dat_match(
                ctx,
                DatWalkState {
                    pos: next_pos,
                    cursor: next_cursor,
                    syl_count: next_syl_count,
                    initial_steps: state.initial_steps + 1,
                    typo_count: state.typo_count,
                    edge_cost: next_cost,
                    has_initial: true,
                    has_partial_tail: state.has_partial_tail,
                    is_exact_all: false,
                    has_habitual_alias: state.has_habitual_alias,
                },
            );
        }
        ctx.key_buf.truncate(saved_len);
    }
}

fn handle_dat_partial_edge(ctx: &mut DatSearchCtx<'_>, state: &DatWalkState, edge: &SyllableEdge) {
    if edge.end != ctx.lattice.len || state.syl_count == 0 {
        return;
    }
    let Some(partial_prefix) = edge.syllables.first() else {
        return;
    };
    let mut cursor = state.cursor;
    if state.syl_count > 0 {
        let Some(sep_c) = cursor.step_byte(&ctx.reader, b'\'') else {
            return;
        };
        cursor = sep_c;
    }
    let mut branches = Vec::with_capacity(32);
    cursor.collect_syllable_branches(&ctx.reader, partial_prefix, 32, &mut branches);

    let next_cost = state.edge_cost.saturating_add(edge.cost);
    let next_syl_count = state.syl_count + 1;
    let saved_len = ctx.key_buf.len();

    for (syl, next_cursor) in branches {
        if syl.len() <= partial_prefix.len() {
            continue;
        }
        if let Some(key_idx) = next_cursor.terminal_value(&ctx.reader) {
            if state.syl_count > 0 {
                ctx.key_buf.push('\'');
            }
            ctx.key_buf.push_str(&syl);
            emit_dat_candidates(
                ctx,
                key_idx as usize,
                MatchMeta {
                    end: edge.end,
                    syllable_count: next_syl_count,
                    typo_count: state.typo_count,
                    edge_cost: next_cost,
                    has_initial: state.has_initial,
                    has_partial_tail: true,
                    is_exact_all: false,
                    has_habitual_alias: state.has_habitual_alias,
                },
            );
            ctx.key_buf.truncate(saved_len);
        }
    }
}

pub(super) struct MatchMeta {
    pub end: usize,
    pub syllable_count: usize,
    pub typo_count: u8,
    pub edge_cost: u32,
    pub has_initial: bool,
    pub has_partial_tail: bool,
    pub is_exact_all: bool,
    pub has_habitual_alias: bool,
}

fn emit_dat_candidates(ctx: &mut DatSearchCtx<'_>, key_idx: usize, meta: MatchMeta) {
    let mut shared_key: Option<Arc<str>> = None;
    let key_buf_str = ctx.key_buf.as_str();
    let results = &mut *ctx.results;
    let filter_rare = ctx.options.filter_rare_ext_chars;
    ctx.mmap.for_each_candidate_by_key_idx(
        key_idx,
        Some(0),
        Some(ctx.options.top_k_per_key),
        |word, raw_score, word_id| {
            if filter_rare && is_rare_ext_char(word) {
                return;
            }
            let matched_key = Arc::clone(shared_key.get_or_insert_with(|| Arc::from(key_buf_str)));
            let char_count = word.chars().count();
            let freq = if meta.has_habitual_alias {
                calibrate_habitual_alias_score(raw_score, char_count == 1)
            } else {
                raw_score
            };
            results.push(RawPhraseMatch {
                end: meta.end,
                word_id,
                char_len: char_count.min(u8::MAX as usize) as u8,
                freq,
                matched_key,
                syllable_count: meta.syllable_count,
                typo_count: meta.typo_count,
                edge_cost: meta.edge_cost,
                has_initial: meta.has_initial,
                has_partial_tail: meta.has_partial_tail,
                is_exact_all: meta.is_exact_all,
            });
        },
    );
}
