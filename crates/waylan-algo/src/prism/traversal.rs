//! 棱镜音节图 DAG 深度优先遍历与容错虚边探索模块
//!
//! # 设计原理
//! - **实现初衷**：将递归深度优先搜索（DFS）、FSA 正向最大匹配、相邻字母倒置与高斯误触虚边探索从 `PrismEngine` 门面中解耦。
//! - **核心优势**：职责高内聚，收敛递归状态上下文；限制递归深度（<=32）与全局分支数（<=64），物理杜绝栈溢出与计算爆炸。
//! - **代价与局限**：在搜索递归中通过借用切片流转。

use std::str;

use crate::prism::alias::get_habitual_alias_targets;
use crate::prism::fsa::PrismFsa;
use crate::prism::fuzzy::PrismFuzzy;
use crate::prism::types::{PathOrigin, SegmentPath, TypoExploration, STANDARD_INITIALS};
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
    /// 本轮搜索的容错虚边强度
    pub exploration: TypoExploration,
    pub current: &'a mut Vec<String>,
    pub current_raw: &'a mut Vec<String>,
    pub results: &'a mut Vec<SegmentPath>,
}

/// 探索模糊音音节匹配扩展分支
fn explore_fuzzy_syllables(
    ctx: &mut DfsContext<'_>,
    params: &DfsExploreParams<'_>,
    matched_full: &mut bool,
) {
    if !ctx.fuzzy.has_active_rules() {
        return;
    }
    for len in (1..=params.max_len).rev() {
        let end = params.start + len;
        if !ctx.input.is_char_boundary(end) {
            continue;
        }
        let slice = &ctx.input[params.start..end];
        if PrismFuzzy::is_fuzzy_valid(slice, ctx.fuzzy)
            && !params.fsa_matches.iter().any(|m| m.0 == slice)
        {
            *matched_full = true;
            let mut step_cost = 12;
            if let Some(prev) = ctx.current.last() {
                if PrismFuzzy::is_zero_initial_borrowing_split(prev, slice) {
                    step_cost += 35;
                }
            }
            ctx.current.push(slice.to_string());
            ctx.current_raw.push(slice.to_string());
            dfs_dag(ctx, end, params.current_cost + step_cost, params.origin);
            ctx.current.pop();
            ctx.current_raw.pop();
        }
    }
}

/// 按容错强度分档探索虚边：拼音方案确定性改写规则与物理键盘紧邻按键高斯误触
///
/// 两类虚边的可信度不对等，故由 [`TypoExploration`] 分离启用：确定性改写命中即正解，正常击键流
/// 亦参与探索；邻键误触属概率猜测，仅在异常流全量容错阶段启用，避免污染正常打字结果。
fn explore_typo_edges(ctx: &mut DfsContext<'_>, params: &DfsExploreParams<'_>) {
    if ctx.exploration == TypoExploration::Strict || ctx.results.len() >= 48 {
        return;
    }

    // 跨音节尾字母共享漏按边（如 zhonguo -> zhong + guo），即使前缀已命中长音节仍可探索
    if ctx.exploration.allows_spatial() && params.origin == PathOrigin::Exact {
        explore_shared_boundary_missing_char(ctx, params);
    }

    if params.fsa_matches.iter().any(|m| m.1 >= 4) {
        return;
    }

    // 复制共享引用，使切片派生的借用不跨越对 ctx 的可变借用
    let input = ctx.input;
    let typo_rules = ctx.typo_rules;
    let allows_spatial = ctx.exploration.allows_spatial();

    for len in (2..=params.max_len).rev() {
        let end = params.start + len;
        if !input.is_char_boundary(end) {
            continue;
        }
        let slice = &input[params.start..end];

        let mut corrections: Vec<(String, u32, PathOrigin)> = Vec::with_capacity(6);
        for rule_alt in get_typo_corrections_with_rules(slice, typo_rules) {
            corrections.push((rule_alt, 16, PathOrigin::RuleNormalized));
        }
        if allows_spatial {
            for (spatial_syl, spatial_cost) in get_spatial_typo_corrections(slice, 25) {
                if !corrections.iter().any(|(s, _, _)| *s == spatial_syl) {
                    corrections.push((spatial_syl, spatial_cost, PathOrigin::SpatialTypo));
                }
            }
        }

        for (std_syl, edit_cost, edge_origin) in corrections {
            if !params.fsa_matches.iter().any(|m| m.0 == std_syl) {
                ctx.current.push(std_syl);
                ctx.current_raw.push(slice.to_string());
                dfs_dag(
                    ctx,
                    end,
                    params.current_cost + 10 + edit_cost,
                    params.origin.max(edge_origin),
                );
                ctx.current.pop();
                ctx.current_raw.pop();
            }
        }
    }
}

/// 探索跨音节边界共享字母导致的漏按边（如 `zhonguo` 中 `g` 被前音节占用，实际应为 `zhong + guo`）
fn explore_shared_boundary_missing_char(ctx: &mut DfsContext<'_>, params: &DfsExploreParams<'_>) {
    let fsa = PrismFsa::global();
    for &(std_syl, len) in params.fsa_matches.iter().rev() {
        if len < 3 {
            continue;
        }
        let end = params.start + len;
        if end >= ctx.input.len() || !ctx.input.is_char_boundary(end - 1) {
            continue;
        }
        // 将当前音节最后一个字母同时借给下一音节作为首字母（等价于用户少按一次该边界字母）
        let next_matches = fsa.match_syllables(ctx.input, end - 1);
        for &(next_syl, next_len) in next_matches.iter().rev() {
            if next_len < 2 {
                continue;
            }
            let total_end = end - 1 + next_len;
            if !ctx.input.is_char_boundary(total_end) {
                continue;
            }
            ctx.current.push(std_syl.to_string());
            ctx.current_raw.push(std_syl.to_string());
            ctx.current.push(next_syl.to_string());
            ctx.current_raw.push(ctx.input[end..total_end].to_string());
            dfs_dag(
                ctx,
                total_end,
                params.current_cost + 20 + 14,
                params.origin.max(PathOrigin::SpatialTypo),
            );
            ctx.current.pop();
            ctx.current_raw.pop();
            ctx.current.pop();
            ctx.current_raw.pop();
        }
    }
}

/// 探索声母简拼混合转移边
fn explore_initials_transitions(
    ctx: &mut DfsContext<'_>,
    start: usize,
    current_cost: u32,
    origin: PathOrigin,
) {
    for &initial in STANDARD_INITIALS {
        if ctx.input[start..].starts_with(initial) {
            let len = initial.len();
            if start + len < ctx.input.len() {
                ctx.current.push(initial.to_string());
                ctx.current_raw.push(initial.to_string());
                dfs_dag(ctx, start + len, current_cost + 25, origin);
                ctx.current.pop();
                ctx.current_raw.pop();
            } else if !ctx.current.is_empty() {
                ctx.results.push(SegmentPath {
                    syllables: ctx.current.clone(),
                    raw_syllables: ctx.current_raw.clone(),
                    remaining: initial.to_string(),
                    cost: current_cost + 30,
                    origin,
                });
            }
        }
    }
}

/// 深度优先搜索构建音节与声母混合 DAG 路径
///
/// `origin` 记录抵达当前节点所经虚边的来源性质：标准音节与声母转移边保持原值，
/// 容错虚边按 [`PathOrigin`] 强弱序取较大者，故混用归一与邻键虚边的路径最终记为邻键档。
pub fn dfs_dag(ctx: &mut DfsContext<'_>, start: usize, current_cost: u32, origin: PathOrigin) {
    if start >= ctx.input.len() {
        push_completed_path(ctx, current_cost, origin);
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
        origin,
        max_len,
        fsa_matches: &fsa_matches,
    };

    // 1.1 ~ 2. 标准音节、模糊音、容错虚边与声母简拼转移边的全量分支探索
    let matched_full = explore_all_branches(ctx, &params, fsa);

    // 3. 若当前处于末尾且无法凑成完整音节，但属于合法前缀，记录为带有 remaining 的候选路径
    push_remainder_path(ctx, &params, matched_full);
}

/// DAG 深度优先搜索的分支探索参数（收敛标准音节、模糊音、紧邻误触等探索类辅助函数的入参数量）
struct DfsExploreParams<'a> {
    /// 当前搜索位置（输入串中的字符偏移）
    start: usize,
    /// 当前累计代价
    current_cost: u32,
    /// 抵达该节点所经虚边的来源性质（非严格档不再叠加容错虚边）
    origin: PathOrigin,
    /// 当前剩余输入可匹配的最大音节长度
    max_len: usize,
    /// 编译型有限状态机给出的标准音节匹配结果
    fsa_matches: &'a [(&'static str, usize)],
}

/// 在当前搜索位置探索全部候选分支，返回本轮是否命中过完整音节
///
/// 探索顺序固定为：标准完整音节 -> 模糊音音节 -> 相邻按键倒置容错 -> 容错虚边（按强度分档，
/// 见 [`explore_typo_edges`]）-> 声母简拼转移。
fn explore_all_branches(
    ctx: &mut DfsContext<'_>,
    params: &DfsExploreParams<'_>,
    fsa: &PrismFsa,
) -> bool {
    // 1. 优先尝试标准完整音节
    let mut matched_full = explore_exact_syllables(ctx, params);

    // 1.2 模糊音音节匹配扩展
    explore_fuzzy_syllables(ctx, params, &mut matched_full);

    // 1.3 容错虚边：相邻按键倒置（支持音节内 (0,1)..=(4,5) 交换与跨音节边界交换）
    if ctx.exploration.allows_spatial()
        && params.origin == PathOrigin::Exact
        && params.start + 1 < ctx.input.len()
        && ctx.results.len() < 48
    {
        explore_transpose_typo(ctx, params.start, params.current_cost, fsa);
    }

    // 1.4 容错虚边：拼音方案确定性改写规则与物理键盘紧邻按键高斯误触
    if params.origin == PathOrigin::Exact {
        explore_typo_edges(ctx, params);
    }

    // 1.5 探索拼音习惯音与叹词音别名衍生分支（如 en -> n, ng; me -> m; mu -> m）
    if params.origin <= PathOrigin::RuleNormalized {
        explore_habitual_aliases(ctx, params, &mut matched_full);
    }

    // 2. 声母简拼混合转移边
    explore_initials_transitions(ctx, params.start, params.current_cost, params.origin);

    matched_full
}

/// 探索拼音习惯音与叹词音别名衍生分支（如 en -> n, ng; me -> m; mu -> m）
fn explore_habitual_aliases(
    ctx: &mut DfsContext<'_>,
    params: &DfsExploreParams<'_>,
    matched_full: &mut bool,
) {
    for len in (1..=params.max_len).rev() {
        let end = params.start + len;
        if !ctx.input.is_char_boundary(end) {
            continue;
        }
        let slice = &ctx.input[params.start..end];
        if let Some((targets, penalty)) = get_habitual_alias_targets(slice) {
            *matched_full = true;
            for &target in targets {
                ctx.current.push(target.to_string());
                ctx.current_raw.push(slice.to_string());
                dfs_dag(
                    ctx,
                    end,
                    params.current_cost + penalty,
                    params.origin.max(PathOrigin::RuleNormalized),
                );
                ctx.current.pop();
                ctx.current_raw.pop();
            }
        }
    }
}

/// 输入已完全消费时，将当前音节序列记录为一条完整切分路径
fn push_completed_path(ctx: &mut DfsContext<'_>, current_cost: u32, origin: PathOrigin) {
    if !ctx.current.is_empty() {
        ctx.results.push(SegmentPath {
            syllables: ctx.current.clone(),
            raw_syllables: ctx.current_raw.clone(),
            remaining: String::new(),
            cost: current_cost,
            origin,
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
            raw_syllables: ctx.current_raw.clone(),
            remaining: remaining.to_string(),
            cost: params.current_cost + 50,
            origin: params.origin,
        });
    }
}

/// 探索标准完整音节路径并返回是否命中过完整音节
fn explore_exact_syllables(ctx: &mut DfsContext<'_>, params: &DfsExploreParams<'_>) -> bool {
    let mut matched_full = false;
    for &(std_syl, len) in params.fsa_matches.iter().rev() {
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
        ctx.current_raw.push(std_syl.to_string());
        dfs_dag(
            ctx,
            params.start + len,
            params.current_cost + step_cost,
            params.origin,
        );
        ctx.current.pop();
        ctx.current_raw.pop();
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
                let slice = &ctx.input[start..start + len];
                ctx.current.push(std_syl.to_string());
                ctx.current_raw.push(slice.to_string());
                dfs_dag(
                    ctx,
                    start + len,
                    current_cost + 10 + 16,
                    PathOrigin::SpatialTypo,
                );
                ctx.current.pop();
                ctx.current_raw.pop();
            } else if len == idx2 {
                let next_matches = fsa.match_syllables(swapped_str, len);
                for &(next_syl, next_len) in next_matches.iter().rev() {
                    let total_len = len + next_len;
                    if ctx.input.is_char_boundary(start + total_len) {
                        let slice1 = &ctx.input[start..start + len];
                        let slice2 = &ctx.input[start + len..start + total_len];
                        ctx.current.push(std_syl.to_string());
                        ctx.current_raw.push(slice1.to_string());
                        ctx.current.push(next_syl.to_string());
                        ctx.current_raw.push(slice2.to_string());
                        dfs_dag(
                            ctx,
                            start + total_len,
                            current_cost + 20 + 16,
                            PathOrigin::SpatialTypo,
                        );
                        ctx.current.pop();
                        ctx.current_raw.pop();
                        ctx.current.pop();
                        ctx.current_raw.pop();
                    }
                }
            }
        }
    }
}
