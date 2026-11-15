//! 基于双数组 Trie (DAT) 的音节格同步词组匹配器
//!
//! # 设计原理
//! - **实现初衷**：在统一音节格的任意字节起点出发，沿音节边同步推进 `DatCursor`，
//!   直接匹配 1..=6 音节的词典词条。遇到混简拼声母边 (`Initial`) 或末尾半成品边 (`PartialTail`)
//!   时直接在当前 DAT 节点展开真实存在的音节分支，彻底消除旧版 `acronym_entries` 的 Top-K 截断问题。
//! - **核心优势**：零多余堆分配，无效前缀在 Trie 层立即剪枝；全量检索与测试均统一运行在零拷贝 DAT 视图上。
//! - **代价与局限**：单次词匹配最多跨越 6 个音节与 4 个混简拼步进，以防止极端输入组合扩散。

use super::alias::calibrate_habitual_alias_score;
use super::lattice::{EdgeKind, SyllableEdge, SyllableLattice};
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
    /// 命中的规范拼音键（如 `"wan'quan'ke'yi"`）
    pub matched_key: String,
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
        let mut results = Vec::new();
        if start_pos >= lattice.len {
            return (results, start_pos);
        }

        let Some(mmap) = &table.compact_mmap else {
            return (results, start_pos);
        };
        let reader = mmap.get_dat_reader();
        let mut ctx = DatSearchCtx {
            lattice,
            mmap,
            reader,
            options,
            start_pos,
            max_reach: start_pos,
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
            key_parts: Vec::with_capacity(6),
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
    start_pos: usize,
    max_reach: usize,
    results: &'a mut Vec<PhraseMatch>,
}

#[derive(Clone)]
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
    key_parts: Vec<String>,
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
    let mut next_parts = state.key_parts.clone();
    for (idx, syl) in edge.syllables.iter().enumerate() {
        if state.syl_count + idx > 0 {
            let Some(sep_c) = cursor.step_byte(&ctx.reader, b'\'') else {
                return;
            };
            cursor = sep_c;
        }
        let Some(syl_c) = cursor.step_str(&ctx.reader, syl) else {
            return;
        };
        cursor = syl_c;
        next_parts.push(syl.clone());
    }

    ctx.max_reach = ctx.max_reach.max(edge.end);
    let next_syl_count = state.syl_count + edge.syllables.len();
    let next_typo = state.typo_count.saturating_add(edge.typo_count);
    let next_cost = state.edge_cost.saturating_add(edge.cost);
    let is_exact = state.is_exact_all && edge.kind == EdgeKind::Exact;
    let has_alias = state.has_habitual_alias || edge.kind == EdgeKind::RuleNormalized;

    if let Some(key_idx) = cursor.terminal_value(&ctx.reader) {
        let matched_key = next_parts.join("'");
        emit_dat_candidates(
            ctx,
            key_idx as usize,
            MatchMeta {
                end: edge.end,
                matched_key,
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
                key_parts: next_parts,
            },
        );
    }
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

    for (syl, next_cursor) in branches {
        let mut next_parts = state.key_parts.clone();
        next_parts.push(syl);
        if next_syl_count >= 2 {
            if let Some(key_idx) = next_cursor.terminal_value(&ctx.reader) {
                let matched_key = next_parts.join("'");
                emit_dat_candidates(
                    ctx,
                    key_idx as usize,
                    MatchMeta {
                        end: edge.end,
                        matched_key,
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
                    key_parts: next_parts,
                },
            );
        }
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

    for (syl, next_cursor) in branches {
        if syl.len() <= partial_prefix.len() {
            continue;
        }
        if let Some(key_idx) = next_cursor.terminal_value(&ctx.reader) {
            let mut next_parts = state.key_parts.clone();
            next_parts.push(syl);
            emit_dat_candidates(
                ctx,
                key_idx as usize,
                MatchMeta {
                    end: edge.end,
                    matched_key: next_parts.join("'"),
                    syllable_count: next_syl_count,
                    typo_count: state.typo_count,
                    edge_cost: next_cost,
                    has_initial: state.has_initial,
                    has_partial_tail: true,
                    is_exact_all: false,
                    has_habitual_alias: state.has_habitual_alias,
                },
            );
        }
    }
}

pub(super) struct MatchMeta {
    pub end: usize,
    pub matched_key: String,
    pub syllable_count: usize,
    pub typo_count: u8,
    pub edge_cost: u32,
    pub has_initial: bool,
    pub has_partial_tail: bool,
    pub is_exact_all: bool,
    pub has_habitual_alias: bool,
}

fn emit_dat_candidates(ctx: &mut DatSearchCtx<'_>, key_idx: usize, meta: MatchMeta) {
    let Some(entries) =
        ctx.mmap
            .get_candidates_by_key_idx(key_idx, Some(0), Some(ctx.options.top_k_per_key))
    else {
        return;
    };
    for (word, raw_score, word_id) in entries {
        if ctx.options.filter_rare_ext_chars && is_rare_ext_char(&word) {
            continue;
        }
        let freq = if meta.has_habitual_alias {
            calibrate_habitual_alias_score(raw_score, word.chars().count() == 1)
        } else {
            raw_score
        };
        ctx.results.push(PhraseMatch {
            start: ctx.start_pos,
            end: meta.end,
            word,
            word_id: Some(word_id),
            freq,
            matched_key: meta.matched_key.clone(),
            syllable_count: meta.syllable_count,
            typo_count: meta.typo_count,
            edge_cost: meta.edge_cost,
            has_initial: meta.has_initial,
            has_partial_tail: meta.has_partial_tail,
            is_exact_all: meta.is_exact_all,
        });
    }
}
