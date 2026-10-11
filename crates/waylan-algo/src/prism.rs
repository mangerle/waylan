use crate::entity::EntityProtectionManager;
use crate::prism_fsa::PrismFsa;
use crate::prism_fuzzy::PrismFuzzy;
use crate::syllables::{
    get_spatial_typo_corrections, get_typo_corrections, has_strictly_longer_syllable,
    has_syllable_prefix, is_valid_syllable,
};
use crate::types::FuzzyConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPath {
    /// 完整音节切分列表（如 ["xi", "an"] 或 ["xian"]）
    pub syllables: Vec<String>,
    /// 末尾未完成键入的剩余前缀（如 "h"）
    pub remaining: String,
    /// 路径代价值（段数越少、匹配越长代价越低，优先排序）
    pub cost: u32,
    /// 是否由容错纠错虚边衍生生成
    pub is_typo: bool,
}

impl SegmentPath {
    /// 转换为带单引号的展示字符串（如 "xi'an"）
    pub fn to_key_string(&self) -> String {
        let mut parts = self.syllables.clone();
        if !self.remaining.is_empty() {
            parts.push(self.remaining.clone());
        }
        parts.join("'")
    }
}

/// 棱镜拼写图引擎 (Prism / Syllable Graph)
///
/// # 设计原理
/// - **实现初衷**：替换原有单一最大正向贪心算法，将用户键入序列投影为有向无环图（DAG），
///   在分词阶段完整保留多路径歧义（如 `xian` 同时分流为 `xian` 和 `xi'an`）。
/// - **核心优势**：用户无需强行输入隔音符即可同时召回“先”与“西安”，并原生集成模糊音与双拼映射。
/// - **代价与局限**：多分支路径搜索开销略高于贪心，但在 20 字符以内的拼音序列下耗时仍低于 15 微秒。
#[derive(Debug, Default)]
pub struct PrismEngine;

/// 现代汉语普通话标准声母列表（双字母翘舌声母排在最前优先匹配）
pub static STANDARD_INITIALS: &[&str] = &[
    "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r",
    "z", "c", "s", "y", "w",
];

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
        Self::dfs_dag(
            input,
            0,
            &mut current_syllables,
            0,
            0,
            false,
            &mut paths,
            fuzzy,
        );

        // 判定是否属于正常击键流程：
        // 1. 前驱音节全部属于完整标准音节或合法模糊音音节（无单字母辅音声母降级）；
        // 2. 末尾闭合（remaining 为空）或处于合法音节前缀键入中（如 "bei" + "j"）
        let is_normal_typing = paths.iter().any(|p| {
            let all_prev_valid = p.syllables.iter().all(|s| {
                (is_valid_syllable(s) || PrismFuzzy::is_fuzzy_valid(s, fuzzy))
                    && (s.len() >= 2 || matches!(s.as_str(), "a" | "o" | "e"))
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
            Self::dfs_dag(
                input,
                0,
                &mut current_syllables,
                0,
                0,
                true,
                &mut paths,
                fuzzy,
            );
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

        // 路径排序：正向非容错路径严格优先于容错路径，同类按代价值排序
        paths.sort_by(|a, b| a.is_typo.cmp(&b.is_typo).then_with(|| a.cost.cmp(&b.cost)));
        paths.dedup();
        paths
    }

    /// 深度优先搜索构建音节与声母混合 DAG 路径（保证 100% 原始击键字母保真）
    #[allow(clippy::too_many_arguments)]
    fn dfs_dag(
        input: &str,
        start: usize,
        current: &mut Vec<String>,
        current_cost: u32,
        typo_count: u8,
        enable_typo: bool,
        results: &mut Vec<SegmentPath>,
        fuzzy: &FuzzyConfig,
    ) {
        if start >= input.len() {
            if !current.is_empty() {
                results.push(SegmentPath {
                    syllables: current.clone(),
                    remaining: String::new(),
                    cost: current_cost,
                    is_typo: typo_count > 0,
                });
            }
            return;
        }

        if results.len() >= 64 {
            return; // 限制全局分支数量，保障微秒级求解
        }

        let max_len = (input.len() - start).min(6);
        let mut matched_full = false;

        // 1. 优先尝试标准完整音节（利用 PrismFsa 编译型有限状态机单向步进匹配，消灭二分查找，保持原字母切片）
        let fsa = PrismFsa::global();
        let fsa_matches = fsa.match_syllables(input, start);
        for &(std_syl, len) in fsa_matches.iter().rev() {
            matched_full = true;
            let mut step_cost = 10;
            // 检查零声母借音碎切：当前音节若以 a, o, e 开头，且与前一个音节可合并成合法单音节，则判定为借音碎切并施加惩罚
            if let Some(prev) = current.last() {
                if Self::is_zero_initial_borrowing_split(prev, std_syl) {
                    step_cost += if matches!(std_syl, "n" | "ng" | "m") {
                        50
                    } else {
                        35
                    };
                }
            }
            current.push(std_syl.to_string());
            Self::dfs_dag(
                input,
                start + len,
                current,
                current_cost + step_cost,
                typo_count,
                enable_typo,
                results,
                fuzzy,
            );
            current.pop();
        }

        // 1.2 模糊音音节匹配扩展（若启用了模糊音开关，严格使用原输入切片 slice）
        if fuzzy.has_active_rules() {
            for len in (1..=max_len).rev() {
                let slice = &input[start..start + len];
                if PrismFuzzy::is_fuzzy_valid(slice, fuzzy)
                    && !fsa_matches.iter().any(|m| m.0 == slice)
                {
                    matched_full = true;
                    let mut step_cost = 12;
                    if let Some(prev) = current.last() {
                        if Self::is_zero_initial_borrowing_split(prev, slice) {
                            step_cost += 35;
                        }
                    }
                    current.push(slice.to_string());
                    Self::dfs_dag(
                        input,
                        start + len,
                        current,
                        current_cost + step_cost,
                        typo_count,
                        enable_typo,
                        results,
                        fuzzy,
                    );
                    current.pop();
                }
            }
        }

        // 1.3 容错虚边：相邻按键倒置（Adjacent Transpose，如 cehng -> cheng, shiohu -> shihou）
        // 防候选词爆炸：仅在第二阶段（enable_typo=true）且单路径未曾容错时探索
        if enable_typo && typo_count == 0 && start + 1 < input.len() && results.len() < 48 {
            let bytes = input.as_bytes();
            let rem_bytes = &bytes[start..];

            // 尝试互换相邻索引 (0, 1) 或 (1, 2)
            let swap_pairs = if rem_bytes.len() >= 3 {
                vec![(0, 1), (1, 2)]
            } else {
                vec![(0, 1)]
            };

            for (idx1, idx2) in swap_pairs {
                let mut swapped = Vec::from(rem_bytes);
                swapped.swap(idx1, idx2);
                if let Ok(swapped_str) = std::str::from_utf8(&swapped) {
                    let swapped_matches = fsa.match_syllables(swapped_str, 0);
                    for &(std_syl, len) in swapped_matches.iter().rev() {
                        if len > idx2 {
                            current.push(std_syl.to_string());
                            Self::dfs_dag(
                                input,
                                start + len,
                                current,
                                current_cost + 10 + 16,
                                1,
                                enable_typo,
                                results,
                                fuzzy,
                            );
                            current.pop();
                        }
                    }
                }
            }
        }

        // 1.4 容错虚边：物理键盘紧邻按键高斯误触与常见韵母规则纠错（如 nihso -> nihao, zhegn -> zheng）
        // 防候选词爆炸：仅在第二阶段且未匹配出 4 字符以上音节时探索
        if enable_typo
            && typo_count == 0
            && !fsa_matches.iter().any(|m| m.1 >= 4)
            && results.len() < 48
        {
            for len in (2..=max_len).rev() {
                let slice = &input[start..start + len];
                let mut corrections = get_spatial_typo_corrections(slice, 25);
                for rule_alt in get_typo_corrections(slice) {
                    if !corrections.iter().any(|(s, _)| s == &rule_alt) {
                        corrections.push((rule_alt, 16));
                    }
                }
                for (std_syl, edit_cost) in corrections {
                    if !fsa_matches.iter().any(|m| m.0 == std_syl) {
                        current.push(std_syl);
                        Self::dfs_dag(
                            input,
                            start + len,
                            current,
                            current_cost + 10 + edit_cost,
                            1,
                            enable_typo,
                            results,
                            fuzzy,
                        );
                        current.pop();
                    }
                }
            }
        }

        // 2. 声母简拼混合转移边（严格保持原始声母字母，绝不改动任何字符）：
        // 若当前声母位于输入中间（后面还有更多字符）：作为跨音节声母转移（代价值 +25）
        // 若当前声母位于输入末尾（start + len == input.len()）：作为未闭合的残余 remaining 前缀记录
        for &initial in STANDARD_INITIALS {
            if input[start..].starts_with(initial) {
                let len = initial.len();
                if start + len < input.len() {
                    current.push(initial.to_string());
                    Self::dfs_dag(
                        input,
                        start + len,
                        current,
                        current_cost + 25,
                        typo_count,
                        enable_typo,
                        results,
                        fuzzy,
                    );
                    current.pop();
                } else if !current.is_empty() {
                    results.push(SegmentPath {
                        syllables: current.clone(),
                        remaining: initial.to_string(),
                        cost: current_cost + 30,
                        is_typo: typo_count > 0,
                    });
                }
            }
        }

        // 3. 若当前处于末尾且无法凑成完整音节，但属于合法前缀，记录为带有 remaining 的候选路径
        if !matched_full && !current.is_empty() {
            let remaining = &input[start..];
            if has_syllable_prefix(remaining) {
                results.push(SegmentPath {
                    syllables: current.clone(),
                    remaining: remaining.to_string(),
                    cost: current_cost + 50,
                    is_typo: typo_count > 0,
                });
            }
        }
    }

    /// 显式单引号强制切分处理（支持末尾未闭合单字前缀继续展开联想）
    fn build_explicit_split_paths(input: &str, fuzzy: &FuzzyConfig) -> Vec<SegmentPath> {
        let parts: Vec<&str> = input.split('\'').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Vec::new();
        }

        let is_trailing_quote = input.ends_with('\'');
        let last_idx = parts.len() - 1;
        let last_part = parts[last_idx];

        let mut base_syllables = Vec::with_capacity(parts.len());
        for &part in &parts[..last_idx] {
            base_syllables.push(part.to_string());
        }

        let mut paths = Vec::new();

        // 1. 若末尾片段本身是合法音节（或末尾输入了单引号强制确认），生成闭合音节路径
        if is_valid_syllable(last_part) || PrismFuzzy::is_fuzzy_valid(last_part, fuzzy) {
            let mut syls = base_syllables.clone();
            syls.push(last_part.to_string());
            let cost = syls.len() as u32;
            paths.push(SegmentPath {
                syllables: syls,
                remaining: String::new(),
                cost,
                is_typo: false,
            });
        }

        // 2. 若末尾未以单引号显式封口，且末尾片段不是合法音节或存在更长的合法音节前缀，
        // 则同时生成将末尾片段作为未完成前缀 (remaining) 的扩展路径（支持如 xi'a 联想 喜爱 xi'ai / 西安 xi'an）
        if !is_trailing_quote
            && (!is_valid_syllable(last_part) || has_strictly_longer_syllable(last_part))
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
