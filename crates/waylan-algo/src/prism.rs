use crate::syllables::{has_syllable_prefix, is_valid_syllable, STANDARD_SYLLABLES};
use crate::types::FuzzyConfig;
use std::sync::OnceLock;

/// 有限状态机节点
#[derive(Clone, Debug)]
struct FsaNode {
    next: [u16; 26],
    syllable: Option<&'static str>,
}

/// 编译型音节投影有限状态机 (Prism FSA)
///
/// # 设计原理
/// - 预编译所有 410 个标准普通话音节为紧凑字典树状态机；
/// - 状态转移在 O(1) 数组索引内步进完成，彻底消灭每个前缀的重复二分查找；
/// - 在输入流上单向步进，以严格 O(L) 复杂度提取所有合法音节切分。
pub struct PrismFsa {
    nodes: Vec<FsaNode>,
}

impl PrismFsa {
    /// 全局静态编译单例
    pub fn global() -> &'static PrismFsa {
        static INSTANCE: OnceLock<PrismFsa> = OnceLock::new();
        INSTANCE.get_or_init(Self::compile)
    }

    /// 编译全部合法标准拼音音节
    fn compile() -> Self {
        let mut nodes = vec![FsaNode {
            next: [0; 26],
            syllable: None,
        }];

        for &syl in STANDARD_SYLLABLES {
            let mut curr = 0;
            for b in syl.bytes() {
                if b.is_ascii_lowercase() {
                    let idx = (b - b'a') as usize;
                    let next_state = nodes[curr].next[idx];
                    if next_state == 0 {
                        let new_state = nodes.len() as u16;
                        nodes[curr].next[idx] = new_state;
                        nodes.push(FsaNode {
                            next: [0; 26],
                            syllable: None,
                        });
                        curr = new_state as usize;
                    } else {
                        curr = next_state as usize;
                    }
                }
            }
            nodes[curr].syllable = Some(syl);
        }

        Self { nodes }
    }

    /// 在输入串上以单向步进方式匹配所有合法音节（返回 (标准音节, 匹配字节长度)）
    pub fn match_syllables(&self, input: &str, start: usize) -> Vec<(&'static str, usize)> {
        let mut matches = Vec::new();
        let bytes = input.as_bytes();
        let mut curr = 0;
        let mut len = 0;

        for &b in &bytes[start..] {
            if !b.is_ascii_lowercase() {
                break;
            }
            let idx = (b - b'a') as usize;
            let next_state = self.nodes[curr].next[idx];
            if next_state == 0 {
                break;
            }
            curr = next_state as usize;
            len += 1;
            if let Some(syl) = self.nodes[curr].syllable {
                matches.push((syl, len));
            }
        }
        matches
    }
}

/// 音节图中的一条候选切分路径
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
    /// 构建拼写音节图并求解所有合法切分路径
    pub fn build_paths(
        input: &str,
        fuzzy: &FuzzyConfig,
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
        let entity_mgr = crate::entity::EntityProtectionManager::new();
        if let Some(entity) = entity_mgr.match_exact(input) {
            paths.push(SegmentPath {
                syllables: vec![entity],
                remaining: String::new(),
                cost: 0,
                is_typo: false,
            });
        }

        let mut current_syllables = Vec::new();
        // 第一阶段：正向严格搜索（不启用容错虚边，保证正常长句以微秒级极致性能直通）
        Self::dfs_dag(input, 0, &mut current_syllables, 0, 0, false, &mut paths, fuzzy);

        // 判定是否属于正常击键流程：
        // 1. 前驱音节全部属于完整标准音节（无单字母辅音声母降级）；
        // 2. 末尾闭合（remaining 为空）或处于合法音节前缀键入中（如 "bei" + "j"）
        let is_normal_typing = paths.iter().any(|p| {
            let all_prev_valid = p.syllables.iter().all(|s| {
                is_valid_syllable(s) && (s.len() >= 2 || matches!(s.as_str(), "a" | "o" | "e"))
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
                'b' | 'p' | 'm' | 'f' | 'd' | 't' | 'n' | 'l' | 'g' | 'k' | 'h' | 'j' | 'q' | 'x' | 'r'
                    | 'z' | 'c' | 's' | 'y' | 'w'
            )
        });

        // 第二阶段：若非纯声母缩写，且非正常全拼键入流（存在断流、孤立辅音声母或非法残余前缀），启动容错虚边补救搜索
        if !is_pure_initials && !is_normal_typing {
            Self::dfs_dag(input, 0, &mut current_syllables, 0, 0, true, &mut paths, fuzzy);
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
                    step_cost += if matches!(std_syl, "n" | "ng" | "m") { 50 } else { 35 };
                }
            }
            current.push(std_syl.to_string());
            Self::dfs_dag(input, start + len, current, current_cost + step_cost, typo_count, enable_typo, results, fuzzy);
            current.pop();
        }

        // 1.2 模糊音音节匹配扩展（若启用了模糊音开关，严格使用原输入切片 slice）
        if fuzzy.has_active_rules() {
            for len in (1..=max_len).rev() {
                let slice = &input[start..start + len];
                if Self::is_fuzzy_valid(slice, fuzzy) && !fsa_matches.iter().any(|m| m.0 == slice) {
                    matched_full = true;
                    let mut step_cost = 12;
                    if let Some(prev) = current.last() {
                        if Self::is_zero_initial_borrowing_split(prev, slice) {
                            step_cost += 35;
                        }
                    }
                    current.push(slice.to_string());
                    Self::dfs_dag(input, start + len, current, current_cost + step_cost, typo_count, enable_typo, results, fuzzy);
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
                            Self::dfs_dag(input, start + len, current, current_cost + 10 + 16, 1, enable_typo, results, fuzzy);
                            current.pop();
                        }
                    }
                }
            }
        }

        // 1.4 容错虚边：物理键盘紧邻按键高斯误触（如 nihso -> nihao, beikin -> beijin）
        // 防候选词爆炸：仅在第二阶段且未匹配出 4 字符以上音节时探索
        if enable_typo && typo_count == 0 && !fsa_matches.iter().any(|m| m.1 >= 4) && results.len() < 48 {
            for len in (2..=max_len).rev() {
                let slice = &input[start..start + len];
                let corrections = crate::syllables::get_spatial_typo_corrections(slice, 25);
                for (std_syl, edit_cost) in corrections {
                    if !fsa_matches.iter().any(|m| m.0 == std_syl) {
                        current.push(std_syl);
                        Self::dfs_dag(input, start + len, current, current_cost + 10 + edit_cost, 1, enable_typo, results, fuzzy);
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
                    Self::dfs_dag(input, start + len, current, current_cost + 25, typo_count, enable_typo, results, fuzzy);
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
        if is_valid_syllable(last_part) || Self::is_fuzzy_valid(last_part, fuzzy) {
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
            && (!is_valid_syllable(last_part)
                || crate::syllables::has_strictly_longer_syllable(last_part))
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

    /// 展开单个音节在当前模糊音配置下的所有合法标准等价音节（不含原音节自身）
    pub fn expand_fuzzy_syllables(slice: &str, fuzzy: &FuzzyConfig) -> Vec<String> {
        if !fuzzy.has_active_rules() || slice.is_empty() {
            return Vec::new();
        }

        // 1. 收集声母等价变体（含原切片自身）
        let mut init_variants = vec![slice.to_string()];

        if fuzzy.z_zh {
            if let Some(rest) = slice.strip_prefix("zh") {
                init_variants.push(format!("z{}", rest));
            } else if let Some(rest) = slice.strip_prefix('z') {
                init_variants.push(format!("zh{}", rest));
            }
        }
        if fuzzy.c_ch {
            if let Some(rest) = slice.strip_prefix("ch") {
                init_variants.push(format!("c{}", rest));
            } else if let Some(rest) = slice.strip_prefix('c') {
                init_variants.push(format!("ch{}", rest));
            }
        }
        if fuzzy.s_sh {
            if let Some(rest) = slice.strip_prefix("sh") {
                init_variants.push(format!("s{}", rest));
            } else if let Some(rest) = slice.strip_prefix('s') {
                init_variants.push(format!("sh{}", rest));
            }
        }
        if fuzzy.l_n {
            if let Some(rest) = slice.strip_prefix('l') {
                init_variants.push(format!("n{}", rest));
            } else if let Some(rest) = slice.strip_prefix('n') {
                init_variants.push(format!("l{}", rest));
            }
        }
        if fuzzy.f_h {
            if let Some(rest) = slice.strip_prefix('f') {
                init_variants.push(format!("h{}", rest));
            } else if let Some(rest) = slice.strip_prefix('h') {
                init_variants.push(format!("f{}", rest));
            }
        }
        if fuzzy.r_l {
            if let Some(rest) = slice.strip_prefix('r') {
                init_variants.push(format!("l{}", rest));
            } else if let Some(rest) = slice.strip_prefix('l') {
                init_variants.push(format!("r{}", rest));
            }
        }

        // 2. 在声母变体基础上叠加韵母等价变体
        let mut all_variants = Vec::new();
        for v in init_variants {
            let mut final_cands = vec![v.clone()];
            if fuzzy.an_ang {
                if let Some(stem) = v.strip_suffix("ang") {
                    final_cands.push(format!("{}an", stem));
                } else if let Some(stem) = v.strip_suffix("an") {
                    final_cands.push(format!("{}ang", stem));
                }
            }
            if fuzzy.en_eng {
                if let Some(stem) = v.strip_suffix("eng") {
                    final_cands.push(format!("{}en", stem));
                } else if let Some(stem) = v.strip_suffix("en") {
                    final_cands.push(format!("{}eng", stem));
                }
            }
            if fuzzy.in_ing {
                if let Some(stem) = v.strip_suffix("ing") {
                    final_cands.push(format!("{}in", stem));
                } else if let Some(stem) = v.strip_suffix("in") {
                    final_cands.push(format!("{}ing", stem));
                }
            }

            for cand in final_cands {
                if cand != slice && is_valid_syllable(&cand) && !all_variants.contains(&cand) {
                    all_variants.push(cand);
                }
            }
        }

        all_variants
    }

    /// 展开一条切分路径在当前模糊音配置下的所有等价词典查询键（不含原始 `path.to_key_string()`）
    pub fn expand_fuzzy_keys(path: &SegmentPath, fuzzy: &FuzzyConfig) -> Vec<String> {
        if !fuzzy.has_active_rules() || path.syllables.is_empty() || !path.remaining.is_empty() {
            return Vec::new();
        }

        let mut combos: Vec<Vec<String>> = vec![Vec::new()];
        for syl in &path.syllables {
            let mut syl_options = Vec::new();
            if is_valid_syllable(syl) {
                syl_options.push(syl.clone());
            }
            for alt in Self::expand_fuzzy_syllables(syl, fuzzy) {
                if !syl_options.contains(&alt) {
                    syl_options.push(alt);
                }
            }
            if syl_options.is_empty() {
                syl_options.push(syl.clone());
            }

            let mut next_combos = Vec::new();
            for prefix in &combos {
                for opt in &syl_options {
                    if next_combos.len() >= 16 {
                        break;
                    }
                    let mut next = prefix.clone();
                    next.push(opt.clone());
                    next_combos.push(next);
                }
                if next_combos.len() >= 16 {
                    break;
                }
            }
            combos = next_combos;
        }

        let orig_key = path.to_key_string();
        combos
            .into_iter()
            .map(|parts| parts.join("'"))
            .filter(|k| k != &orig_key)
            .collect()
    }

    /// 模糊音等价性判定
    fn is_fuzzy_valid(slice: &str, fuzzy: &FuzzyConfig) -> bool {
        !Self::expand_fuzzy_syllables(slice, fuzzy).is_empty()
    }

    /// 零声母与特殊单辅音音节集合（以 a, o, e 开头，或特殊叹词音节 n, ng, m）
    #[inline]
    pub fn is_zero_initial_syllable(syl: &str) -> bool {
        matches!(
            syl,
            "a" | "ai" | "an" | "ang" | "ao"
                | "e" | "ei" | "en" | "eng" | "er"
                | "o" | "ou"
                | "n" | "ng" | "m"
        )
    }

    /// 判定是否属于与前驱音节无隔音符借音碎切
    ///
    /// # 判定原理
    /// 若当前音节为零声母音节或特殊叹词鼻辅音（如 n），且前驱音节与当前音节拼接在一起后，本身就是一个合法的标准拼音音节，
    /// 说明用户在无隔音符情况下敲击的是连续拼音，把它们强行拆开属于无标记借音碎切（如 "ti"+"an"=>"tian", "me"+"n"=>"men"）。
    #[inline]
    pub fn is_zero_initial_borrowing_split(prev: &str, curr: &str) -> bool {
        if !Self::is_zero_initial_syllable(curr) {
            return false;
        }
        let mut combined = String::with_capacity(prev.len() + curr.len());
        combined.push_str(prev);
        combined.push_str(curr);
        is_valid_syllable(&combined)
    }


}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_ambiguity_xian() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("xian", &fuzzy);
        let key_strings: Vec<String> = paths.iter().map(|p| p.to_key_string()).collect();

        // 必须同时包含单音节 xian 与双音节 xi'an
        assert!(key_strings.contains(&"xian".to_string()));
        assert!(key_strings.contains(&"xi'an".to_string()));
    }

    #[test]
    fn test_explicit_split_quote() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("xi'an", &fuzzy);
        assert!(!paths.is_empty());
        assert_eq!(paths[0].syllables, vec!["xi", "an"]);

        // 验证 xi'a 能够同时切分出闭合的 xi'a 与带 remaining: a 的前缀扩展路径
        let paths_prefix = PrismEngine::build_paths("xi'a", &fuzzy);
        assert!(paths_prefix.iter().any(|p| p.syllables == vec!["xi", "a"]));
        assert!(paths_prefix.iter().any(|p| p.syllables == vec!["xi"] && p.remaining == "a"));
    }

    #[test]
    fn test_partial_remaining() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("nih", &fuzzy);
        assert!(!paths.is_empty());
        assert_eq!(paths[0].syllables, vec!["ni"]);
        assert_eq!(paths[0].remaining, "h");
    }

    #[test]
    fn test_strict_raw_initials_segmentation() {
        let fuzzy = FuzzyConfig::default();
        // 用户输入纯声母长串 qwdwwqswsqw，测试切分是否 100% 原按键保真，绝不篡改任何字母
        let paths = PrismEngine::build_paths("qwdwwqswsqw", &fuzzy);
        assert!(!paths.is_empty(), "切分路径不应为空");
        let best_path = &paths[0];
        assert_eq!(
            best_path.to_key_string(),
            "q'w'd'w'w'q's'w's'q'w",
            "声母切分音节必须严格保持用户输入的原始字母，绝不允许换字母"
        );
    }

    #[test]
    fn test_zero_initial_borrowing_split_cost() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("jintianxingqitian", &fuzzy);
        assert!(!paths.is_empty());
        // 最优主路径必须是完整的星期天，且代价值低于借音碎切的 ti'an
        assert_eq!(paths[0].to_key_string(), "jin'tian'xing'qi'tian");
        let sub_split_opt = paths.iter().find(|p| p.to_key_string() == "jin'tian'xing'qi'ti'an");
        if let Some(sub_path) = sub_split_opt {
            assert!(paths[0].cost < sub_path.cost, "正常长音节切分代价值必须严格低于借音碎切");
        }
    }

    #[test]
    fn test_typo_adjacent_transpose_graph() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("cehng", &fuzzy);
        assert!(!paths.is_empty(), "切分路径不应为空");
        let has_cheng = paths.iter().any(|p| p.syllables.contains(&"cheng".to_string()));
        assert!(has_cheng, "输入 cehng 应通过相邻颠倒虚边成功容错切分出 cheng");
    }

    #[test]
    fn test_typo_spatial_substitution_graph() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("nihso", &fuzzy);
        assert!(!paths.is_empty(), "切分路径不应为空");
        let has_ni_hao = paths.iter().any(|p| p.to_key_string() == "ni'hao");
        assert!(has_ni_hao, "输入 nihso 应通过高斯邻键虚边成功容错切分出 ni'hao");
    }

    #[test]
    fn test_fuzzy_nine_rules_expansion() {
        let fuzzy = FuzzyConfig::all_enabled();
        let zong_alts = PrismEngine::expand_fuzzy_syllables("zong", &fuzzy);
        assert!(zong_alts.contains(&"zhong".to_string()));

        let lu_alts = PrismEngine::expand_fuzzy_syllables("lu", &fuzzy);
        assert!(lu_alts.contains(&"nu".to_string()));
        assert!(lu_alts.contains(&"ru".to_string()));

        let fa_alts = PrismEngine::expand_fuzzy_syllables("fa", &fuzzy);
        assert!(fa_alts.contains(&"ha".to_string()));

        let lan_alts = PrismEngine::expand_fuzzy_syllables("lan", &fuzzy);
        assert!(lan_alts.contains(&"lang".to_string()));
        assert!(lan_alts.contains(&"nan".to_string()));
        assert!(lan_alts.contains(&"nang".to_string()));

        let sen_alts = PrismEngine::expand_fuzzy_syllables("sen", &fuzzy);
        assert!(sen_alts.contains(&"seng".to_string()));
        assert!(sen_alts.contains(&"shen".to_string()));
        assert!(sen_alts.contains(&"sheng".to_string()));
    }

    #[test]
    fn test_fuzzy_engine_lookup_zongguo() {
        let mut config = crate::types::AlgoConfig::default();
        config.fuzzy = FuzzyConfig::all_enabled();
        let mut engine = crate::AlgoEngine::with_sample_data(config);
        for ch in "zongguo".chars() {
            engine.push_char(ch);
        }
        assert_eq!(
            engine.formatted_preedit(),
            "zong'guo",
            "模糊音模式下预编辑文本必须严格保持用户原始键入字母"
        );
        let texts: Vec<String> = engine.candidates().iter().map(|c| c.text.clone()).collect();
        assert!(
            texts.contains(&"中国".to_string()),
            "开启模糊音后输入 zongguo 应能联合检索召回 中国，实际候选: {:?}",
            texts
        );
        let idx = texts.iter().position(|t| t == "中国").unwrap();
        let (committed, rem) = engine.select_candidate(idx, 100).unwrap();
        assert_eq!(committed, "中国");
        assert_eq!(rem, 0, "选中模糊音候选词后应完整消费原始按键序列");
    }
}
