//! 棱镜音节图 DAG 深度优先遍历与容错虚边探索模块
//!
//! # 设计原理
//! - **实现初衷**：将递归深度优先搜索（DFS）、FSA 正向最大匹配、相邻字母倒置与高斯误触虚边探索从 `PrismEngine` 门面中解耦。
//! - **核心优势**：职责高内聚，收敛递归状态上下文；限制递归深度（<=32）与全局分支数（<=64），物理杜绝栈溢出与计算爆炸。
//! - **代价与局限**：在搜索递归中通过借用切片流转。

use std::str;

use crate::prism::fsa::PrismFsa;
use crate::prism::fuzzy::PrismFuzzy;
use crate::prism::types::{SegmentPath, STANDARD_INITIALS};
use crate::syllables::{
    get_spatial_typo_corrections, get_typo_corrections_with_rules, has_syllable_prefix,
};
use crate::types::FuzzyConfig;
use waylan_core::config::TypoCorrectionRules;

/// 音节图深度优先搜索上下文（收敛递归参数）
pub struct DfsContext<'a> {
    pub input: &'a str,
    pub fuzzy: &'a FuzzyConfig,
    pub typo_rules: &'a TypoCorrectionRules,
    pub enable_typo: bool,
    pub current: &'a mut Vec<String>,
    pub results: &'a mut Vec<SegmentPath>,
}

/// 探索模糊音音节匹配扩展分支
fn explore_fuzzy_syllables(
    ctx: &mut DfsContext<'_>,
    start: usize,
    current_cost: u32,
    typo_count: u8,
    max_len: usize,
    fsa_matches: &[(&str, usize)],
    matched_full: &mut bool,
) {
    if !ctx.fuzzy.has_active_rules() {
        return;
    }
    for len in (1..=max_len).rev() {
        let end = start + len;
        if !ctx.input.is_char_boundary(end) {
            continue;
        }
        let slice = &ctx.input[start..end];
        if PrismFuzzy::is_fuzzy_valid(slice, ctx.fuzzy) && !fsa_matches.iter().any(|m| m.0 == slice)
        {
            *matched_full = true;
            let mut step_cost = 12;
            if let Some(prev) = ctx.current.last() {
                if PrismFuzzy::is_zero_initial_borrowing_split(prev, slice) {
                    step_cost += 35;
                }
            }
            ctx.current.push(slice.to_string());
            dfs_dag(ctx, end, current_cost + step_cost, typo_count);
            ctx.current.pop();
        }
    }
}

/// 探索物理键盘紧邻按键高斯误触与常见韵母规则纠错
fn explore_spatial_and_rule_typo(
    ctx: &mut DfsContext<'_>,
    start: usize,
    current_cost: u32,
    max_len: usize,
    fsa_matches: &[(&str, usize)],
) {
    if !ctx.enable_typo || fsa_matches.iter().any(|m| m.1 >= 4) || ctx.results.len() >= 48 {
        return;
    }

    for len in (2..=max_len).rev() {
        let end = start + len;
        if !ctx.input.is_char_boundary(end) {
            continue;
        }
        let slice = &ctx.input[start..end];
        let mut corrections = get_spatial_typo_corrections(slice, 25);
        for rule_alt in get_typo_corrections_with_rules(slice, ctx.typo_rules) {
            if !corrections.iter().any(|(s, _)| s == &rule_alt) {
                corrections.push((rule_alt, 16));
            }
        }
        for (std_syl, edit_cost) in corrections {
            if !fsa_matches.iter().any(|m| m.0 == std_syl) {
                ctx.current.push(std_syl);
                dfs_dag(ctx, end, current_cost + 10 + edit_cost, 1);
                ctx.current.pop();
            }
        }
    }
}

/// 探索声母简拼混合转移边
fn explore_initials_transitions(
    ctx: &mut DfsContext<'_>,
    start: usize,
    current_cost: u32,
    typo_count: u8,
) {
    for &initial in STANDARD_INITIALS {
        if ctx.input[start..].starts_with(initial) {
            let len = initial.len();
            if start + len < ctx.input.len() {
                ctx.current.push(initial.to_string());
                dfs_dag(ctx, start + len, current_cost + 25, typo_count);
                ctx.current.pop();
            } else if !ctx.current.is_empty() {
                ctx.results.push(SegmentPath {
                    syllables: ctx.current.clone(),
                    remaining: initial.to_string(),
                    cost: current_cost + 30,
                    is_typo: typo_count > 0,
                });
            }
        }
    }
}

/// 深度优先搜索构建音节与声母混合 DAG 路径（保证 100% 原始击键字母保真）
pub fn dfs_dag(ctx: &mut DfsContext<'_>, start: usize, current_cost: u32, typo_count: u8) {
    if start >= ctx.input.len() {
        push_completed_path(ctx, current_cost, typo_count);
        return;
    }

    if !ctx.input.is_char_boundary(start) {
        return;
    }

    if ctx.results.len() >= 64 || ctx.current.len() >= 32 {
        return; // 限制全局分支数量与最大递归深度（<=32 音节），保障微秒级求解并防爆栈
    }

    let max_len = (ctx.input.len() - start).min(6);

    // 1. 优先尝试标准完整音节（利用 PrismFsa 编译型有限状态机单向步进匹配，消灭二分查找，保持原字母切片）
    let fsa = PrismFsa::global();
    let fsa_matches = fsa.match_syllables(ctx.input, start);
    let params = DfsExploreParams {
        start,
        current_cost,
        typo_count,
        max_len,
        fsa_matches: &fsa_matches,
    };

    // 1.1 ~ 2. 标准音节、模糊音、容错虚边与声母简拼转移边的全量分支探索
    let matched_full = explore_all_branches(ctx, &params, fsa);

    // 3. 若当前处于末尾且无法凑成完整音节，但属于合法前缀，记录为带有 remaining 的候选路径
    push_remainder_path(ctx, &params, matched_full);
}

/// DAG 深度优先搜索的分支探索参数（收敛探索类辅助函数的入参数量）
struct DfsExploreParams<'a> {
    /// 当前搜索位置（输入串中的字符偏移）
    start: usize,
    /// 当前累计代价
    current_cost: u32,
    /// 已累计的容错次数
    typo_count: u8,
    /// 当前剩余输入可匹配的最大音节长度
    max_len: usize,
    /// 编译型有限状态机给出的标准音节匹配结果
    fsa_matches: &'a [(&'static str, usize)],
}

/// 在当前搜索位置探索全部候选分支，返回本轮是否命中过完整音节
///
/// 探索顺序固定为：标准完整音节 -> 模糊音音节 -> 相邻按键倒置容错 -> 紧邻误触与规则纠错 -> 声母简拼转移。
fn explore_all_branches(
    ctx: &mut DfsContext<'_>,
    params: &DfsExploreParams<'_>,
    fsa: &PrismFsa,
) -> bool {
    // 1. 优先尝试标准完整音节
    let mut matched_full = explore_exact_syllables(
        ctx,
        params.start,
        params.current_cost,
        params.typo_count,
        params.fsa_matches,
    );

    // 1.2 模糊音音节匹配扩展
    explore_fuzzy_syllables(
        ctx,
        params.start,
        params.current_cost,
        params.typo_count,
        params.max_len,
        params.fsa_matches,
        &mut matched_full,
    );

    // 1.3 容错虚边：相邻按键倒置（支持音节内 (0,1)..=(4,5) 交换与跨音节边界交换）
    if ctx.enable_typo
        && params.typo_count == 0
        && params.start + 1 < ctx.input.len()
        && ctx.results.len() < 48
    {
        explore_transpose_typo(ctx, params.start, params.current_cost, fsa);
    }

    // 1.4 容错虚边：物理键盘紧邻按键高斯误触与常见韵母规则纠错
    if params.typo_count == 0 {
        explore_spatial_and_rule_typo(
            ctx,
            params.start,
            params.current_cost,
            params.max_len,
            params.fsa_matches,
        );
    }

    // 2. 声母简拼混合转移边
    explore_initials_transitions(ctx, params.start, params.current_cost, params.typo_count);

    matched_full
}

/// 输入已完全消费时，将当前音节序列记录为一条完整切分路径
fn push_completed_path(ctx: &mut DfsContext<'_>, current_cost: u32, typo_count: u8) {
    if !ctx.current.is_empty() {
        ctx.results.push(SegmentPath {
            syllables: ctx.current.clone(),
            remaining: String::new(),
            cost: current_cost,
            is_typo: typo_count > 0,
        });
    }
}

/// 末尾无法凑成完整音节但属于合法音节前缀时，记录为带 remaining 的候选路径
fn push_remainder_path(
    ctx: &mut DfsContext<'_>,
    params: &DfsExploreParams<'_>,
    matched_full: bool,
) {
    if matched_full || ctx.current.is_empty() {
        return;
    }
    let remaining = &ctx.input[params.start..];
    if has_syllable_prefix(remaining) {
        ctx.results.push(SegmentPath {
            syllables: ctx.current.clone(),
            remaining: remaining.to_string(),
            cost: params.current_cost + 50,
            is_typo: params.typo_count > 0,
        });
    }
}

/// 探索标准完整音节路径并返回是否命中过完整音节
fn explore_exact_syllables(
    ctx: &mut DfsContext<'_>,
    start: usize,
    current_cost: u32,
    typo_count: u8,
    fsa_matches: &[(&'static str, usize)],
) -> bool {
    let mut matched_full = false;
    for &(std_syl, len) in fsa_matches.iter().rev() {
        matched_full = true;
        let mut step_cost = 10;
        // 检查零声母借音碎切：当前音节若以 a, o, e 开头，且与前一个音节可合并成合法单音节，则判定为借音碎切并施加惩罚
        if let Some(prev) = ctx.current.last() {
            if PrismFuzzy::is_zero_initial_borrowing_split(prev, std_syl) {
                step_cost += if matches!(std_syl, "n" | "ng" | "m") {
                    50
                } else {
                    35
                };
            }
        }
        ctx.current.push(std_syl.to_string());
        dfs_dag(ctx, start + len, current_cost + step_cost, typo_count);
        ctx.current.pop();
    }
    matched_full
}

/// 探索相邻字母交换容错路径（覆盖单音节内 (0,1)..=(4,5) 交换及跨音节边界首尾字母倒置）
pub fn explore_transpose_typo(
    ctx: &mut DfsContext<'_>,
    start: usize,
    current_cost: u32,
    fsa: &PrismFsa,
) {
    const SWAP_PAIRS: [(usize, usize); 5] = [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5)];
    let rem_bytes = &ctx.input.as_bytes()[start..];

    for &(idx1, idx2) in &SWAP_PAIRS {
        if idx2 >= rem_bytes.len() {
            break;
        }
        if !rem_bytes[idx1].is_ascii_alphabetic() || !rem_bytes[idx2].is_ascii_alphabetic() {
            continue;
        }
        let mut swapped = Vec::from(rem_bytes);
        swapped.swap(idx1, idx2);
        let Ok(swapped_str) = str::from_utf8(&swapped) else {
            continue;
        };
        let swapped_matches = fsa.match_syllables(swapped_str, 0);
        for &(std_syl, len) in swapped_matches.iter().rev() {
            if len > idx2 && ctx.input.is_char_boundary(start + len) {
                ctx.current.push(std_syl.to_string());
                dfs_dag(ctx, start + len, current_cost + 10 + 16, 1);
                ctx.current.pop();
            } else if len == idx2 {
                let next_matches = fsa.match_syllables(swapped_str, len);
                for &(next_syl, next_len) in next_matches.iter().rev() {
                    let total_len = len + next_len;
                    if ctx.input.is_char_boundary(start + total_len) {
                        ctx.current.push(std_syl.to_string());
                        ctx.current.push(next_syl.to_string());
                        dfs_dag(ctx, start + total_len, current_cost + 20 + 16, 1);
                        ctx.current.pop();
                        ctx.current.pop();
                    }
                }
            }
        }
    }
}
