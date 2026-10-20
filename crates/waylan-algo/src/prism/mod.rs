use crate::entity::EntityProtectionManager;
use crate::syllables::{has_strictly_longer_syllable, has_syllable_prefix, is_valid_syllable};
use crate::types::FuzzyConfig;
use waylan_core::config::TypoCorrectionRules;
pub mod fsa;
pub mod fuzzy;
pub mod traversal;
pub mod types;

#[cfg(test)]
mod tests;

pub use fsa::PrismFsa;
pub use fuzzy::PrismFuzzy;
pub use traversal::{dfs_dag, DfsContext};
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
        if let Some(mgr) = entity_mgr {
            if let Some(entity) = mgr.match_exact(input) {
                paths.push(SegmentPath {
                    syllables: vec![entity],
                    remaining: String::new(),
                    cost: 0,
                    is_typo: false,
                });
            }
        }

        let mut current_syllables = Vec::new();
        // 第一阶段：正向严格搜索（不启用容错虚边，保证正常长句以微秒级极致性能直通）
        {
            let mut ctx = DfsContext {
                input,
                fuzzy,
                typo_rules,
                enable_typo: false,
                current: &mut current_syllables,
                results: &mut paths,
            };
            dfs_dag(&mut ctx, 0, 0, 0);
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
        if !is_pure_initials && !is_normal_typing && typo_rules.enable {
            let mut ctx = DfsContext {
                input,
                fuzzy,
                typo_rules,
                enable_typo: true,
                current: &mut current_syllables,
                results: &mut paths,
            };
            dfs_dag(&mut ctx, 0, 0, 0);
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
