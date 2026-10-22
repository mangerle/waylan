use crate::entity::EntityProtectionManager;
use crate::syllables::{has_strictly_longer_syllable, has_syllable_prefix, is_valid_syllable};
use crate::types::FuzzyConfig;
use waylan_core::config::TypoCorrectionRules;
pub mod alias;
pub mod fsa;
pub mod fuzzy;
pub mod traversal;
pub mod types;

#[cfg(test)]
mod tests;

pub use alias::{
    calibrate_habitual_alias_score, get_habitual_alias_targets, is_habitual_alias_key,
    is_habitual_alias_target, HabitualAliasRule, HABITUAL_ALIAS_RULES,
};
pub use fsa::PrismFsa;
pub use fuzzy::PrismFuzzy;
pub use traversal::{dfs_dag, DfsContext};
pub use types::{PathOrigin, SegmentPath, TypoExploration, STANDARD_INITIALS};

/// 棱镜拼写图引擎 (Prism / Syllable Graph)
///
/// # 设计原理
/// - **实现初衷**：替换原有单一最大正向贪心算法，将用户键入序列投影为有向无环图（DAG），
///   在分词阶段完整保留多路径歧义（如 `xian` 同时分流为 `xian` 和 `xi'an`）。
/// - **核心优势**：用户无需强行输入隔音符即可同时召回“先”与“西安”，并原生集成模糊音与双拼映射。
/// - **代价与局限**：多分支路径搜索开销略高于贪心，但在 20 字符以内的拼音序列下耗时仍低于 15 微秒。
#[derive(Debug, Default)]
pub struct PrismEngine;

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
        Self::build_paths_with_options(input, fuzzy, &TypoCorrectionRules::default(), entity_mgr)
    }

    /// 构建拼写音节图并应用用户配置的拼音纠错规则
    pub fn build_paths_with_options(
        input: &str,
        fuzzy: &FuzzyConfig,
        typo_rules: &TypoCorrectionRules,
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
        Self::push_protected_entity_path(input, entity_mgr, &mut paths);

        let mut current_syllables = Vec::new();
        let params = DfsSearchParams {
            input,
            fuzzy,
            typo_rules,
        };

        // 第一阶段：正向严格搜索（不启用容错虚边，保证正常长句以微秒级极致性能直通）
        Self::run_dfs_search(
            &params,
            TypoExploration::Strict,
            &mut current_syllables,
            &mut paths,
        );

        // 判定是否属于正常击键流程
        let is_normal_typing = Self::is_normal_typing_flow(&paths, fuzzy);

        // 判定是否属于纯声母缩写（所有按键字符均为合法辅音声母）：纯声母缩写绝不触发拼音容错
        let is_pure_initials = Self::is_pure_initials_input(input);

        // 第二阶段：按击键流状态选择容错强度
        // - 纯声母缩写与主开关关闭：完全不探索虚边
        // - 正常全拼击键流：仅探索拼音方案确定性归一虚边，救回 `niou`/`guei`/`luen` 这类
        //   能完整切分但写法非标准的输入，同时不引入邻键猜测噪声
        // - 异常击键流（断流、孤立辅音声母或非法残余前缀）：启用全量容错虚边补救
        let exploration = if is_pure_initials || !typo_rules.enable {
            TypoExploration::Strict
        } else if is_normal_typing {
            TypoExploration::RuleOnly
        } else {
            TypoExploration::Full
        };
        if exploration != TypoExploration::Strict {
            Self::run_dfs_search(&params, exploration, &mut current_syllables, &mut paths);
        }

        // 如果完全无法形成完整音节切分，尝试提取最长可能前缀
        if paths.is_empty() {
            Self::push_unresolved_path(input, &mut paths);
        }

        Self::dedup_best_paths(&mut paths);
        paths
    }

    /// 当输入完全无法形成完整音节切分时，记录一条携带完整剩余串的兜底路径
    fn push_unresolved_path(input: &str, paths: &mut Vec<SegmentPath>) {
        paths.push(SegmentPath {
            syllables: Vec::new(),
            raw_syllables: Vec::new(),
            remaining: input.to_string(),
            cost: 100,
            origin: PathOrigin::Exact,
        });
    }

    /// 在 DAG 上按指定容错强度执行一次深度优先路径搜索
    fn run_dfs_search(
        params: &DfsSearchParams<'_>,
        exploration: TypoExploration,
        current: &mut Vec<String>,
        results: &mut Vec<SegmentPath>,
    ) {
        let mut current_raw = Vec::new();
        let mut ctx = DfsContext {
            input: params.input,
            fuzzy: params.fuzzy,
            typo_rules: params.typo_rules,
            exploration,
            current,
            current_raw: &mut current_raw,
            results,
        };
        dfs_dag(&mut ctx, 0, 0, PathOrigin::Exact);
    }

    /// 若输入精确匹配受保护实体，直接生成零代价原子路径（专有名词与实体保护机制）
    fn push_protected_entity_path(
        input: &str,
        entity_mgr: Option<&EntityProtectionManager>,
        paths: &mut Vec<SegmentPath>,
    ) {
        if let Some(mgr) = entity_mgr {
            if let Some(entity) = mgr.match_exact(input) {
                paths.push(SegmentPath {
                    syllables: vec![entity.clone()],
                    raw_syllables: vec![entity],
                    remaining: String::new(),
                    cost: 0,
                    origin: PathOrigin::Exact,
                });
            }
        }
    }

    /// 判定候选路径集合是否属于正常全拼击键流程
    ///
    /// 正常流程需同时满足：前驱音节均为完整标准音节或合法模糊音音节（无单字母辅音声母降级，
    /// 且无夹在中间的孤立单字母元音碎切），且末尾闭合（remaining 为空）或处于合法音节前缀键入中。
    fn is_normal_typing_flow(paths: &[SegmentPath], fuzzy: &FuzzyConfig) -> bool {
        paths
            .iter()
            .any(|p| Self::is_segment_path_normal_flow(p, fuzzy))
    }

    /// 判定单条切分路径是否满足正常全拼击键流程约束
    fn is_segment_path_normal_flow(path: &SegmentPath, fuzzy: &FuzzyConfig) -> bool {
        let has_mid_single_vowel = path.syllables.len() >= 3
            && path.syllables[1..]
                .iter()
                .any(|s| matches!(s.as_str(), "a" | "o" | "e"));
        if has_mid_single_vowel {
            return false;
        }

        let all_prev_valid = path.syllables.iter().all(|s| {
            (is_valid_syllable(s) || PrismFuzzy::is_fuzzy_valid(s, fuzzy))
                && (s.len() >= 2 || matches!(s.as_str(), "a" | "o" | "e" | "n" | "ng" | "m"))
        });
        if !all_prev_valid {
            return false;
        }

        path.remaining.is_empty() || has_syllable_prefix(&path.remaining)
    }

    /// 判定字符是否属于汉语拼音声母（含 y/w 半元音）字母集合
    fn is_initial_consonant(c: char) -> bool {
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
    }

    /// 判定输入序列是否由纯辅音声母字符构成（此类输入绝不触发拼音容错搜索）
    fn is_pure_initials_input(input: &str) -> bool {
        input.chars().all(Self::is_initial_consonant)
    }

    /// 路径排序与全局去重：来源越严格越优先（正向切分 > 确定性归一 > 概率性容错），
    /// 其次代价值与音节数最小优先，相同 (syllables, remaining) 仅保留最优解
    fn dedup_best_paths(paths: &mut Vec<SegmentPath>) {
        paths.sort_by(|a, b| {
            a.origin
                .cmp(&b.origin)
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

    /// 显式单引号强制切分处理（支持多音节子片段自动拆解及末尾未闭合单字前缀展开联想）
    fn build_explicit_split_paths(input: &str, fuzzy: &FuzzyConfig) -> Vec<SegmentPath> {
        let parts: Vec<&str> = input.split('\'').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Vec::new();
        }

        let is_trailing_quote = input.ends_with('\'');
        let last_idx = parts.len() - 1;
        let last_part = parts[last_idx];

        let base_syllables = Self::build_base_syllables(&parts, last_idx, fuzzy);
        let mut paths = Vec::new();
        let last_is_single_valid =
            is_valid_syllable(last_part) || PrismFuzzy::is_fuzzy_valid(last_part, fuzzy);

        let tail = TailSplitParams {
            base_syllables: &base_syllables,
            last_part,
            fuzzy,
            is_trailing_quote,
            last_is_single_valid,
        };

        // 1. 若末尾片段本身是单个合法音节，生成闭合音节路径
        Self::push_tail_closed_paths(&tail, &mut paths);

        // 2. 若末尾未以单引号显式封口，且末尾片段是合法音节前缀（非完整音节或存在更长合法音节），
        // 则生成将末尾片段作为未完成前缀 (remaining) 的扩展路径（支持如 xi'a 联想 喜爱 xi'ai / 西安 xi'an）
        Self::push_tail_prefix_paths(&tail, &mut paths);

        if paths.is_empty() {
            let syls: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
            paths.push(SegmentPath {
                raw_syllables: syls.clone(),
                syllables: syls,
                remaining: String::new(),
                cost: parts.len() as u32,
                origin: PathOrigin::Exact,
            });
        }

        paths
    }

    /// 构建显式切分的前置音节序列（非合法音节的片段尝试自动拆解为多音节子路径）
    fn build_base_syllables(parts: &[&str], last_idx: usize, fuzzy: &FuzzyConfig) -> Vec<String> {
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
        base_syllables
    }

    /// 生成末段为完整音节的闭合路径，以及末段包含多个连续音节时的拆解拼接路径
    fn push_tail_closed_paths(tail: &TailSplitParams<'_>, paths: &mut Vec<SegmentPath>) {
        if tail.last_is_single_valid {
            let mut syls = tail.base_syllables.to_vec();
            syls.push(tail.last_part.to_string());
            let cost = syls.len() as u32;
            paths.push(SegmentPath {
                raw_syllables: syls.clone(),
                syllables: syls,
                remaining: String::new(),
                cost,
                origin: PathOrigin::Exact,
            });
        } else {
            // 1.1 若末尾片段包含多个连续音节（如 "xi'anhui" 中的 "anhui"），调用内部切分逻辑拆解拼接
            for sub in Self::build_paths(tail.last_part, tail.fuzzy) {
                if !sub.syllables.is_empty() {
                    let mut syls = tail.base_syllables.to_vec();
                    syls.extend(sub.syllables);
                    let mut raw_syls = tail.base_syllables.to_vec();
                    raw_syls.extend(sub.raw_syllables);
                    let remaining = if tail.is_trailing_quote && !sub.remaining.is_empty() {
                        syls.push(sub.remaining);
                        String::new()
                    } else {
                        sub.remaining
                    };
                    let cost = (syls.len() as u32).saturating_add(sub.cost / 10);
                    paths.push(SegmentPath {
                        syllables: syls,
                        raw_syllables: raw_syls,
                        remaining,
                        cost,
                        origin: sub.origin,
                    });
                }
            }
        }
    }

    /// 生成将末段作为未完成前缀 (remaining) 的扩展路径（支持如 xi'a 联想 喜爱 xi'ai / 西安 xi'an）
    fn push_tail_prefix_paths(tail: &TailSplitParams<'_>, paths: &mut Vec<SegmentPath>) {
        if !tail.is_trailing_quote
            && has_syllable_prefix(tail.last_part)
            && (!tail.last_is_single_valid || has_strictly_longer_syllable(tail.last_part))
        {
            let cost = (tail.base_syllables.len() + 1) as u32;
            paths.push(SegmentPath {
                syllables: tail.base_syllables.to_vec(),
                raw_syllables: tail.base_syllables.to_vec(),
                remaining: tail.last_part.to_string(),
                cost,
                origin: PathOrigin::Exact,
            });
        }
    }
}

/// DAG 多阶段深度优先搜索的共享输入参数（收敛搜索入口的入参数量）
struct DfsSearchParams<'a> {
    /// 用户原始键入序列
    input: &'a str,
    /// 模糊音规则配置
    fuzzy: &'a FuzzyConfig,
    /// 用户拼音纠错规则
    typo_rules: &'a TypoCorrectionRules,
}

/// 显式切分末段处理的输入参数（收敛末段路径生成函数的入参数量）
struct TailSplitParams<'a> {
    /// 已构建完成的前置音节序列
    base_syllables: &'a [String],
    /// 末段原始文本
    last_part: &'a str,
    /// 模糊音规则配置
    fuzzy: &'a FuzzyConfig,
    /// 输入是否以单引号显式封口
    is_trailing_quote: bool,
    /// 末段本身是否为单个合法音节
    last_is_single_valid: bool,
}

impl PrismEngine {
    /// 按模糊音规则展开单个拼音切片的全部等价变体
    #[inline]
    pub fn expand_fuzzy_syllables(slice: &str, fuzzy: &FuzzyConfig) -> Vec<String> {
        PrismFuzzy::expand_fuzzy_syllables(slice, fuzzy)
    }

    /// 展开整条切分路径可参与词表查询的全部模糊音等价键
    #[inline]
    pub fn expand_fuzzy_keys(path: &SegmentPath, fuzzy: &FuzzyConfig) -> Vec<String> {
        PrismFuzzy::expand_fuzzy_keys(path, fuzzy)
    }

    /// 判定音节是否为零声母音节（以 a/o/e 等元音直接起头）
    #[inline]
    pub fn is_zero_initial_syllable(syl: &str) -> bool {
        PrismFuzzy::is_zero_initial_syllable(syl)
    }

    /// 判定相邻音节之间是否发生"前音节借用韵尾导致后音节退化为零声母"的碎切
    #[inline]
    pub fn is_zero_initial_borrowing_split(prev: &str, curr: &str) -> bool {
        PrismFuzzy::is_zero_initial_borrowing_split(prev, curr)
    }
}
