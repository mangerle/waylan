//! 内存词典表（`LexiconTable::entries`）的回退词边匹配器
//!
//! # 设计原理
//! - **实现初衷**：当测试或轻量模式未挂载二进制 Mmap DAT 时，为 `DatPhraseMatcher` 提供等价语义的词条检索支持。
//! - **核心优势**：保持与 DAT 匹配完全一致的精确、混简拼与严格前缀半成品语义。
//! - **代价与局限**：混简拼/半成品需线性扫描内存词表键集合，仅用于无 Mmap 的单测与小词表环境。

use super::alias::calibrate_habitual_alias_score;
use super::lattice::{EdgeKind, SyllableLattice};
use super::matcher::{MatchMeta, MatcherOptions, PhraseMatch};
use crate::pipeline::is_rare_ext_char;
use crate::table::LexiconTable;

#[derive(Clone)]
pub(super) enum MemPattern {
    Exact(String),
    Prefix(String),
    StrictPrefix(String),
}

#[derive(Clone)]
pub(super) struct MemWalkState {
    pub pos: usize,
    pub syl_count: usize,
    pub initial_steps: usize,
    pub typo_count: u8,
    pub edge_cost: u32,
    pub has_initial: bool,
    pub has_partial_tail: bool,
    pub is_exact_all: bool,
    pub has_habitual_alias: bool,
    pub patterns: Vec<MemPattern>,
}

/// 在内存词典表上从 `start_pos` 递归匹配词边
pub(super) fn dfs_mem_match(
    lattice: &SyllableLattice,
    table: &LexiconTable,
    options: &MatcherOptions,
    start_pos: usize,
    state: MemWalkState,
    results: &mut Vec<PhraseMatch>,
) {
    if state.pos >= lattice.len || state.syl_count >= options.max_syllables {
        return;
    }
    for edge in &lattice.edges_from[state.pos] {
        if state.typo_count.saturating_add(edge.typo_count) > options.max_typos {
            continue;
        }
        if state.syl_count + edge.syllables.len() > options.max_syllables {
            continue;
        }
        if edge.kind == EdgeKind::Initial && state.initial_steps >= options.max_initial_steps {
            continue;
        }
        if edge.kind == EdgeKind::PartialTail && state.syl_count == 0 {
            continue;
        }
        let mut next_patterns = state.patterns.clone();
        for syl in &edge.syllables {
            match edge.kind {
                EdgeKind::Initial => next_patterns.push(MemPattern::Prefix(syl.clone())),
                EdgeKind::PartialTail => next_patterns.push(MemPattern::StrictPrefix(syl.clone())),
                _ => next_patterns.push(MemPattern::Exact(syl.clone())),
            }
        }
        let next_syl_count = state.syl_count + edge.syllables.len();
        let next_typo = state.typo_count.saturating_add(edge.typo_count);
        let next_cost = state.edge_cost.saturating_add(edge.cost);
        let has_initial = state.has_initial || edge.kind == EdgeKind::Initial;
        let has_partial = state.has_partial_tail || edge.kind == EdgeKind::PartialTail;
        let is_exact = state.is_exact_all && edge.kind == EdgeKind::Exact;
        let has_alias = state.has_habitual_alias || edge.kind == EdgeKind::RuleNormalized;

        let is_isolated_initial = edge.kind == EdgeKind::Initial && next_syl_count == 1;
        if !is_isolated_initial {
            emit_mem_candidates(
                table,
                options,
                &next_patterns,
                MatchMeta {
                    end: edge.end,
                    matched_key: String::new(),
                    syllable_count: next_syl_count,
                    typo_count: next_typo,
                    edge_cost: next_cost,
                    has_initial,
                    has_partial_tail: has_partial,
                    is_exact_all: is_exact,
                    has_habitual_alias: has_alias,
                },
                start_pos,
                results,
            );
        }

        let next_pos = lattice.skip_separators(edge.end);
        if edge.kind != EdgeKind::PartialTail
            && next_pos < lattice.len
            && next_syl_count < options.max_syllables
        {
            dfs_mem_match(
                lattice,
                table,
                options,
                start_pos,
                MemWalkState {
                    pos: next_pos,
                    syl_count: next_syl_count,
                    initial_steps: state.initial_steps
                        + usize::from(edge.kind == EdgeKind::Initial),
                    typo_count: next_typo,
                    edge_cost: next_cost,
                    has_initial,
                    has_partial_tail: has_partial,
                    is_exact_all: is_exact,
                    has_habitual_alias: has_alias,
                    patterns: next_patterns,
                },
                results,
            );
        }
    }
}

fn emit_mem_candidates(
    table: &LexiconTable,
    options: &MatcherOptions,
    patterns: &[MemPattern],
    meta: MatchMeta,
    start_pos: usize,
    results: &mut Vec<PhraseMatch>,
) {
    let all_exact = patterns.iter().all(|p| matches!(p, MemPattern::Exact(_)));
    if all_exact {
        let key: String = patterns
            .iter()
            .filter_map(|p| match p {
                MemPattern::Exact(s) => Some(s.as_str()),
                MemPattern::Prefix(_) | MemPattern::StrictPrefix(_) => None,
            })
            .collect::<Vec<_>>()
            .join("'");
        if let Some(entries) = table.entries.get(&key) {
            for (word, raw_score) in entries.iter().take(options.top_k_per_key) {
                if options.filter_rare_ext_chars && is_rare_ext_char(word) {
                    continue;
                }
                let freq = if meta.has_habitual_alias {
                    calibrate_habitual_alias_score(*raw_score, word.chars().count() == 1)
                } else {
                    *raw_score
                };
                results.push(PhraseMatch {
                    start: start_pos,
                    end: meta.end,
                    word: word.clone(),
                    word_id: None,
                    freq,
                    matched_key: key.clone(),
                    syllable_count: meta.syllable_count,
                    typo_count: meta.typo_count,
                    edge_cost: meta.edge_cost,
                    has_initial: meta.has_initial,
                    has_partial_tail: meta.has_partial_tail,
                    is_exact_all: meta.is_exact_all,
                });
            }
        }
        return;
    }

    for (key, entries) in &table.entries {
        if !match_key_patterns(key, patterns) {
            continue;
        }
        for (word, raw_score) in entries.iter().take(options.top_k_per_key) {
            if options.filter_rare_ext_chars && is_rare_ext_char(word) {
                continue;
            }
            results.push(PhraseMatch {
                start: start_pos,
                end: meta.end,
                word: word.clone(),
                word_id: None,
                freq: *raw_score,
                matched_key: key.clone(),
                syllable_count: meta.syllable_count,
                typo_count: meta.typo_count,
                edge_cost: meta.edge_cost,
                has_initial: meta.has_initial,
                has_partial_tail: meta.has_partial_tail,
                is_exact_all: false,
            });
        }
    }
}

fn match_key_patterns(key: &str, patterns: &[MemPattern]) -> bool {
    let mut key_parts = key.split('\'');
    for pat in patterns {
        let Some(k_syl) = key_parts.next() else {
            return false;
        };
        match pat {
            MemPattern::Exact(s) => {
                if k_syl != s {
                    return false;
                }
            }
            MemPattern::Prefix(p) => {
                if !k_syl.starts_with(p) {
                    return false;
                }
            }
            MemPattern::StrictPrefix(p) => {
                if k_syl.len() <= p.len() || !k_syl.starts_with(p) {
                    return false;
                }
            }
        }
    }
    key_parts.next().is_none()
}
