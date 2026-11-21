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
use super::lattice::{EdgeKind, SyllableEdge, SyllableLattice};
use super::matcher_rank::{
    filter_and_truncate_raw_matches, materialize_raw_matches, RawPhraseMatch,
};
pub use super::matcher_rank::{MatcherOptions, PhraseMatch};
use super::weights::ScoringWeights;
use crate::dat::{DatCursor, DatReader};
use crate::pipeline::is_rare_ext_char;
use crate::table::{CompactMmapLexicon, LexiconTable};

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
        filter_and_truncate_raw_matches(
            &mut raw,
            start_pos,
            target_len,
            weights,
            keep_start_extras,
        );

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
            branch_pool: Vec::with_capacity(128),
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

struct DatSearchCtx<'a> {
    lattice: &'a SyllableLattice,
    mmap: &'a CompactMmapLexicon,
    reader: DatReader<'a>,
    options: &'a MatcherOptions,
    max_reach: usize,
    /// DFS 共享可回溯拼音键拼接缓冲，消除每分支 Vec<String> 克隆与 join 堆分配
    key_buf: String,
    /// DFS 共享分支缓冲池，消除递归展开 Initial/PartialTail 时的 Vec 堆分配
    branch_pool: Vec<(String, DatCursor)>,
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
    let base_idx = ctx.branch_pool.len();
    cursor.collect_syllable_branches(&ctx.reader, initial, 32, &mut ctx.branch_pool);
    let end_idx = ctx.branch_pool.len();
    if end_idx > base_idx {
        ctx.max_reach = ctx.max_reach.max(edge.end);
    }

    let next_pos = ctx.lattice.skip_separators(edge.end);
    let next_cost = state.edge_cost.saturating_add(edge.cost);
    let next_syl_count = state.syl_count + 1;
    let saved_len = ctx.key_buf.len();

    for idx in base_idx..end_idx {
        let next_cursor = ctx.branch_pool[idx].1;
        if state.syl_count > 0 {
            ctx.key_buf.push('\'');
        }
        ctx.key_buf.push_str(&ctx.branch_pool[idx].0);
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
    ctx.branch_pool.truncate(base_idx);
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
    let base_idx = ctx.branch_pool.len();
    cursor.collect_syllable_branches(&ctx.reader, partial_prefix, 32, &mut ctx.branch_pool);
    let end_idx = ctx.branch_pool.len();

    let next_cost = state.edge_cost.saturating_add(edge.cost);
    let next_syl_count = state.syl_count + 1;
    let saved_len = ctx.key_buf.len();

    for idx in base_idx..end_idx {
        if ctx.branch_pool[idx].0.len() <= partial_prefix.len() {
            continue;
        }
        let next_cursor = ctx.branch_pool[idx].1;
        if let Some(key_idx) = next_cursor.terminal_value(&ctx.reader) {
            if state.syl_count > 0 {
                ctx.key_buf.push('\'');
            }
            ctx.key_buf.push_str(&ctx.branch_pool[idx].0);
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
    ctx.branch_pool.truncate(base_idx);
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
