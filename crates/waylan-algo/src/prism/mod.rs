use crate::entity::EntityProtectionManager;
use crate::syllables::{
    get_spatial_typo_corrections, get_typo_corrections, has_strictly_longer_syllable,
    has_syllable_prefix, is_valid_syllable,
};
use crate::types::FuzzyConfig;
use std::str;
pub mod fsa;
pub mod fuzzy;
pub mod types;

#[cfg(test)]
mod tests;

pub use fsa::PrismFsa;
pub use fuzzy::PrismFuzzy;
pub use types::{SegmentPath, STANDARD_INITIALS};

/// 棱镜拼写图引擎 (Prism / Syllable Graph)
///
/// # 设计原理
/// - **实现初衷**：替换原有单一最大正向贪心算法，将用户键入序列投影为有向无环图（DAG），
///   在分词阶段完整保留多路径歧义（如 `xian` 同时分流为 `xian` 和 `xi'an`）。
/// - **核心优势**：用户无需强行输入隔音符即可同时召回“先”与“西安”，并原生集成模糊音与双拼映射。
/// - **代价与局限**：多分支路径搜索开销略高于贪心，但在 20 字符以内的拼音序列下耗时仍低于 15 微秒。
#[derive(Debug, Default)]
pub struct PrismEngine;

/// 音节图深度优先搜索上下文（收敛递归参数）
struct DfsContext<'a> {
    input: &'a str,
    fuzzy: &'a FuzzyConfig,
    enable_typo: bool,
    current: &'a mut Vec<String>,
    results: &'a mut Vec<SegmentPath>,
}

impl PrismEngine {
    /// 构建拼写音节图并求解所有合法切分路径（使用默认全局实体保护器）
    pub fn build_paths(input: &str, fuzzy: &FuzzyConfig) -> Vec<SegmentPath> {
        Self::build_paths_with_entities(input, fuzzy, Some(EntityProtectionManager::global()))
    }

    /// 构建拼写音节图并求解所有合法切分路径（支持注入自定义实体保护管理器实例）
    pub fn build_paths_with_entities(
        input: &str,
        fuzzy: &FuzzyConfig,
        entity_mgr: Option<&EntityProtectionManager>,
    ) -> Vec<SegmentPath> {
        if input.is_empty() {
            return Vec::new();
        }

        // 1. 检查是否有用户显式键入的单引号隔音符
        if input.contains('\'') {
            return Self::build_explicit_split_paths(input, fuzzy);
        }

        // 3. 全拼与简拼混合模式：在 DAG 上执行多路径搜索
        let mut paths = Vec::new();

        // 3.1 专有名词与实体保护机制 (NER 原型)：若输入精确匹配受保护实体，直接生成零代价原子路径
        let mgr = entity_mgr.unwrap_or_else(|| EntityProtectionManager::global());
        if let Some(entity) = mgr.match_exact(input) {
            paths.push(SegmentPath {
                syllables: vec![entity],
                remaining: String::new(),
                cost: 0,
                is_typo: false,
            });
        }

        let mut current_syllables = Vec::new();
        // 第一阶段：正向严格搜索（不启用容错虚边，保证正常长句以微秒级极致性能直通）
        {
            let mut ctx = DfsContext {
                input,
                fuzzy,
                enable_typo: false,
                current: &mut current_syllables,
                results: &mut paths,
            };
            Self::dfs_dag(&mut ctx, 0, 0, 0);
        }

        // 判定是否属于正常击键流程：
        // 1. 前驱音节全部属于完整标准音节或合法模糊音音节（无单字母辅音声母降级，且无夹在中间的孤立单字母元音碎切如 shi'o'hu）；
        // 2. 末尾闭合（remaining 为空）或处于合法音节前缀键入中（如 "bei" + "j"）
        let is_normal_typing = paths.iter().any(|p| {
            let has_mid_single_vowel = p.syllables.len() >= 3
                && p.syllables[1..]
                    .iter()
                    .any(|s| matches!(s.as_str(), "a" | "o" | "e"));
            if has_mid_single_vowel {
                return false;
            }
            let all_prev_valid = p.syllables.iter().all(|s| {
                (is_valid_syllable(s) || PrismFuzzy::is_fuzzy_valid(s, fuzzy))
                    && (s.len() >= 2 || matches!(s.as_str(), "a" | "o" | "e" | "n" | "ng" | "m"))
            });
            if !all_prev_valid {
                return false;
            }
            if p.remaining.is_empty() {
                true
            } else {
                has_syllable_prefix(&p.remaining)
            }
        });

        // 判定是否属于纯声母缩写（所有按键字符均为合法辅音声母）：纯声母缩写绝不触发拼音容错
        let is_pure_initials = input.chars().all(|c| {
            matches!(
                c,
                'b' | 'p'
                    | 'm'
                    | 'f'
                    | 'd'
                    | 't'
                    | 'n'
                    | 'l'
                    | 'g'
                    | 'k'
                    | 'h'
                    | 'j'
                    | 'q'
                    | 'x'
                    | 'r'
                    | 'z'
                    | 'c'
                    | 's'
                    | 'y'
                    | 'w'
            )
        });

        // 第二阶段：若非纯声母缩写，且非正常全拼键入流（存在断流、孤立辅音声母或非法残余前缀），启动容错虚边补救搜索
        if !is_pure_initials && !is_normal_typing {
            let mut ctx = DfsContext {
                input,
                fuzzy,
                enable_typo: true,
                current: &mut current_syllables,
                results: &mut paths,
            };
            Self::dfs_dag(&mut ctx, 0, 0, 0);
        }

        // 如果完全无法形成完整音节切分，尝试提取最长可能前缀
        if paths.is_empty() {
            paths.push(SegmentPath {
                syllables: Vec::new(),
                remaining: input.to_string(),
                cost: 100,
                is_typo: false,
            });
        }

        Self::dedup_best_paths(&mut paths);
        paths
    }

    /// 路径排序与全局去重：正向非容错路径优先、代价值与音节数最小优先，相同 (syllables, remaining) 仅保留最优解
    fn dedup_best_paths(paths: &mut Vec<SegmentPath>) {
        paths.sort_by(|a, b| {
            a.is_typo
                .cmp(&b.is_typo)
                .then_with(|| a.cost.cmp(&b.cost))
                .then_with(|| a.syllables.len().cmp(&b.syllables.len()))
        });
        let mut unique: Vec<SegmentPath> = Vec::with_capacity(paths.len());
        for path in paths.drain(..) {
            if !unique
                .iter()
                .any(|u| u.syllables == path.syllables && u.remaining == path.remaining)
            {
                unique.push(path);
            }
        }
        *paths = unique;
    }

    /// 深度优先搜索构建音节与声母混合 DAG 路径（保证 100% 原始击键字母保真）
    fn dfs_dag(ctx: &mut DfsContext<'_>, start: usize, current_cost: u32, typo_count: u8) {
        if start >= ctx.input.len() {
            if !ctx.current.is_empty() {
                ctx.results.push(SegmentPath {
                    syllables: ctx.current.clone(),
                    remaining: String::new(),
                    cost: current_cost,
                    is_typo: typo_count > 0,
                });
            }
            return;
        }

        if !ctx.input.is_char_boundary(start) {
            return;
        }

        if ctx.results.len() >= 64 || ctx.current.len() >= 32 {
            return; // 限制全局分支数量与最大递归深度（<=32 音节），保障微秒级求解并防爆栈
        }

        let max_len = (ctx.input.len() - start).min(6);
        let mut matched_full = false;

        // 1. 优先尝试标准完整音节（利用 PrismFsa 编译型有限状态机单向步进匹配，消灭二分查找，保持原字母切片）
        let fsa = PrismFsa::global();
        let fsa_matches = fsa.match_syllables(ctx.input, start);
        for &(std_syl, len) in fsa_matches.iter().rev() {
            matched_full = true;
            let mut step_cost = 10;
            // 检查零声母借音碎切：当前音节若以 a, o, e 开头，且与前一个音节可合并成合法单音节，则判定为借音碎切并施加惩罚
            if let Some(prev) = ctx.current.last() {
                if Self::is_zero_initial_borrowing_split(prev, std_syl) {
                    step_cost += if matches!(std_syl, "n" | "ng" | "m") {
                        50
                    } else {
                        35
                    };
                }
            }
            ctx.current.push(std_syl.to_string());
            Self::dfs_dag(ctx, start + len, current_cost + step_cost, typo_count);
            ctx.current.pop();
        }

        // 1.2 模糊音音节匹配扩展（若启用了模糊音开关，严格使用原输入切片 slice）
        if ctx.fuzzy.has_active_rules() {
            for len in (1..=max_len).rev() {
                let end = start + len;
                if !ctx.input.is_char_boundary(end) {
                    continue;
                }
                let slice = &ctx.input[start..end];
                if PrismFuzzy::is_fuzzy_valid(slice, ctx.fuzzy)
                    && !fsa_matches.iter().any(|m| m.0 == slice)
                {
                    matched_full = true;
                    let mut step_cost = 12;
                    if let Some(prev) = ctx.current.last() {
                        if Self::is_zero_initial_borrowing_split(prev, slice) {
                            step_cost += 35;
                        }
                    }
                    ctx.current.push(slice.to_string());
                    Self::dfs_dag(ctx, end, current_cost + step_cost, typo_count);
                    ctx.current.pop();
                }
            }
        }

        // 1.3 容错虚边：相邻按键倒置（支持音节内 (0,1)..=(4,5) 交换与跨音节边界交换）
        if ctx.enable_typo
            && typo_count == 0
            && start + 1 < ctx.input.len()
            && ctx.results.len() < 48
        {
            Self::explore_transpose_typo(ctx, start, current_cost, fsa);
        }

        // 1.4 容错虚边：物理键盘紧邻按键高斯误触与常见韵母规则纠错（如 nihso -> nihao, zhegn -> zheng）
        // 防候选词爆炸：仅在第二阶段且未匹配出 4 字符以上音节时探索
        if ctx.enable_typo
            && typo_count == 0
            && !fsa_matches.iter().any(|m| m.1 >= 4)
            && ctx.results.len() < 48
        {
            for len in (2..=max_len).rev() {
                let end = start + len;
                if !ctx.input.is_char_boundary(end) {
                    continue;
                }
                let slice = &ctx.input[start..end];
                let mut corrections = get_spatial_typo_corrections(slice, 25);
                for rule_alt in get_typo_corrections(slice) {
                    if !corrections.iter().any(|(s, _)| s == &rule_alt) {
                        corrections.push((rule_alt, 16));
                    }
                }
                for (std_syl, edit_cost) in corrections {
                    if !fsa_matches.iter().any(|m| m.0 == std_syl) {
                        ctx.current.push(std_syl);
                        Self::dfs_dag(ctx, end, current_cost + 10 + edit_cost, 1);
                        ctx.current.pop();
                    }
                }
            }
        }

        // 2. 声母简拼混合转移边（严格保持原始声母字母，绝不改动任何字符）：
        // 若当前声母位于输入中间（后面还有更多字符）：作为跨音节声母转移（代价值 +25）
        // 若当前声母位于输入末尾（start + len == input.len()）：作为未闭合的残余 remaining 前缀记录
        for &initial in STANDARD_INITIALS {
            if ctx.input[start..].starts_with(initial) {
                let len = initial.len();
                if start + len < ctx.input.len() {
                    ctx.current.push(initial.to_string());
                    Self::dfs_dag(ctx, start + len, current_cost + 25, typo_count);
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

        // 3. 若当前处于末尾且无法凑成完整音节，但属于合法前缀，记录为带有 remaining 的候选路径
        if !matched_full && !ctx.current.is_empty() {
            let remaining = &ctx.input[start..];
            if has_syllable_prefix(remaining) {
                ctx.results.push(SegmentPath {
                    syllables: ctx.current.clone(),
                    remaining: remaining.to_string(),
                    cost: current_cost + 50,
                    is_typo: typo_count > 0,
                });
            }
        }
    }

    /// 探索相邻字母交换容错路径（覆盖单音节内 (0,1)..=(4,5) 交换及跨音节边界首尾字母倒置）
    fn explore_transpose_typo(
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
                    // 音节内部相邻字母倒置（如 cehng -> cheng, zhuagn -> zhuang）
                    ctx.current.push(std_syl.to_string());
                    Self::dfs_dag(ctx, start + len, current_cost + 10 + 16, 1);
                    ctx.current.pop();
                } else if len == idx2 {
                    // 跨音节边界相邻字母倒置（前音节末尾 idx1 与后音节开头 idx2 互换，如 nhiao -> ni'hao, shhiou -> shi'hou）
                    let next_matches = fsa.match_syllables(swapped_str, len);
                    for &(next_syl, next_len) in next_matches.iter().rev() {
                        let total_len = len + next_len;
                        if ctx.input.is_char_boundary(start + total_len) {
                            ctx.current.push(std_syl.to_string());
                            ctx.current.push(next_syl.to_string());
                            Self::dfs_dag(ctx, start + total_len, current_cost + 20 + 16, 1);
                            ctx.current.pop();
                            ctx.current.pop();
                        }
                    }
                }
            }
        }
    }

    /// 显式单引号强制切分处理（支持多音节子片段自动拆解及末尾未闭合单字前缀展开联想）
    fn build_explicit_split_paths(input: &str, fuzzy: &FuzzyConfig) -> Vec<SegmentPath> {
        let parts: Vec<&str> = input.split('\'').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Vec::new();
        }

        let is_trailing_quote = input.ends_with('\'');
        let last_idx = parts.len() - 1;
        let last_part = parts[last_idx];

        let mut base_syllables = Vec::with_capacity(parts.len() + 2);
        for &part in &parts[..last_idx] {
            if is_valid_syllable(part)
                || PrismFuzzy::is_fuzzy_valid(part, fuzzy)
                || STANDARD_INITIALS.contains(&part)
            {
                base_syllables.push(part.to_string());
            } else if let Some(first_sub) = Self::build_paths(part, fuzzy)
                .into_iter()
                .find(|p| !p.syllables.is_empty())
            {
                base_syllables.extend(first_sub.syllables);
                if !first_sub.remaining.is_empty() {
                    base_syllables.push(first_sub.remaining);
                }
            } else {
                base_syllables.push(part.to_string());
            }
        }

        let mut paths = Vec::new();
        let last_is_single_valid =
            is_valid_syllable(last_part) || PrismFuzzy::is_fuzzy_valid(last_part, fuzzy);

        // 1. 若末尾片段本身是单个合法音节，生成闭合音节路径
        if last_is_single_valid {
            let mut syls = base_syllables.clone();
            syls.push(last_part.to_string());
            let cost = syls.len() as u32;
            paths.push(SegmentPath {
                syllables: syls,
                remaining: String::new(),
                cost,
                is_typo: false,
            });
        } else {
            // 1.1 若末尾片段包含多个连续音节（如 "xi'anhui" 中的 "anhui"），调用内部切分逻辑拆解拼接
            for sub in Self::build_paths(last_part, fuzzy) {
                if !sub.syllables.is_empty() {
                    let mut syls = base_syllables.clone();
                    syls.extend(sub.syllables);
                    let remaining = if is_trailing_quote && !sub.remaining.is_empty() {
                        syls.push(sub.remaining);
                        String::new()
                    } else {
                        sub.remaining
                    };
                    let cost = (syls.len() as u32).saturating_add(sub.cost / 10);
                    paths.push(SegmentPath {
                        syllables: syls,
                        remaining,
                        cost,
                        is_typo: sub.is_typo,
                    });
                }
            }
        }

        // 2. 若末尾未以单引号显式封口，且末尾片段是合法音节前缀（非完整音节或存在更长合法音节），
        // 则生成将末尾片段作为未完成前缀 (remaining) 的扩展路径（支持如 xi'a 联想 喜爱 xi'ai / 西安 xi'an）
        if !is_trailing_quote
            && has_syllable_prefix(last_part)
            && (!last_is_single_valid || has_strictly_longer_syllable(last_part))
        {
            let cost = (base_syllables.len() + 1) as u32;
            paths.push(SegmentPath {
                syllables: base_syllables,
                remaining: last_part.to_string(),
                cost,
                is_typo: false,
            });
        }

        if paths.is_empty() {
            paths.push(SegmentPath {
                syllables: parts.iter().map(|s| s.to_string()).collect(),
                remaining: String::new(),
                cost: parts.len() as u32,
                is_typo: false,
            });
        }

        paths
    }
}

impl PrismEngine {
    #[inline]
    pub fn expand_fuzzy_syllables(slice: &str, fuzzy: &FuzzyConfig) -> Vec<String> {
        PrismFuzzy::expand_fuzzy_syllables(slice, fuzzy)
    }

    #[inline]
    pub fn expand_fuzzy_keys(path: &SegmentPath, fuzzy: &FuzzyConfig) -> Vec<String> {
        PrismFuzzy::expand_fuzzy_keys(path, fuzzy)
    }

    #[inline]
    pub fn is_zero_initial_syllable(syl: &str) -> bool {
        PrismFuzzy::is_zero_initial_syllable(syl)
    }

    #[inline]
    pub fn is_zero_initial_borrowing_split(prev: &str, curr: &str) -> bool {
        PrismFuzzy::is_zero_initial_borrowing_split(prev, curr)
    }
}
