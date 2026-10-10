use crate::syllables::{has_syllable_prefix, is_valid_syllable, STANDARD_SYLLABLES};
use crate::types::{DoublePinyinSchema, FuzzyConfig};
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
        double_pinyin: Option<DoublePinyinSchema>,
    ) -> Vec<SegmentPath> {
        if input.is_empty() {
            return Vec::new();
        }

        // 1. 若启用了双拼方案，先进行双拼解码展开
        if let Some(schema) = double_pinyin {
            return Self::decode_double_pinyin(input, schema);
        }

        // 2. 检查是否有用户显式键入的单引号隔音符
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
            });
        }

        let mut current_syllables = Vec::new();
        Self::dfs_dag(input, 0, &mut current_syllables, 0, &mut paths, fuzzy);

        // 如果完全无法形成完整音节切分，尝试提取最长可能前缀
        if paths.is_empty() {
            paths.push(SegmentPath {
                syllables: Vec::new(),
                remaining: input.to_string(),
                cost: 100,
            });
        }

        // 按代价值升序排序（全拼完整度最高、段数适中的排在前）
        paths.sort_by_key(|p| p.cost);
        paths.dedup();
        paths
    }

    /// 深度优先搜索构建音节与声母混合 DAG 路径（保证 100% 原始击键字母保真）
    fn dfs_dag(
        input: &str,
        start: usize,
        current: &mut Vec<String>,
        current_cost: u32,
        results: &mut Vec<SegmentPath>,
        fuzzy: &FuzzyConfig,
    ) {
        if start >= input.len() {
            if !current.is_empty() {
                results.push(SegmentPath {
                    syllables: current.clone(),
                    remaining: String::new(),
                    cost: current_cost,
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
            current.push(std_syl.to_string());
            Self::dfs_dag(input, start + len, current, current_cost + 10, results, fuzzy);
            current.pop();
        }

        // 1.2 模糊音音节匹配扩展（若启用了模糊音开关，严格使用原输入切片 slice）
        if fuzzy.has_active_rules() {
            for len in (1..=max_len).rev() {
                let slice = &input[start..start + len];
                if Self::is_fuzzy_valid(slice, fuzzy) && !fsa_matches.iter().any(|m| m.0 == slice) {
                    matched_full = true;
                    current.push(slice.to_string());
                    Self::dfs_dag(input, start + len, current, current_cost + 12, results, fuzzy);
                    current.pop();
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
                    Self::dfs_dag(input, start + len, current, current_cost + 25, results, fuzzy);
                    current.pop();
                } else if !current.is_empty() {
                    results.push(SegmentPath {
                        syllables: current.clone(),
                        remaining: initial.to_string(),
                        cost: current_cost + 30,
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
                });
            }
        }
    }

    /// 显式单引号强制切分处理
    fn build_explicit_split_paths(input: &str, fuzzy: &FuzzyConfig) -> Vec<SegmentPath> {
        let parts: Vec<&str> = input.split('\'').filter(|s| !s.is_empty()).collect();
        let mut syllables = Vec::new();
        let mut remaining = String::new();

        for (idx, &part) in parts.iter().enumerate() {
            if is_valid_syllable(part) || Self::is_fuzzy_valid(part, fuzzy) {
                syllables.push(part.to_string());
            } else if idx == parts.len().saturating_sub(1) {
                remaining = part.to_string();
            } else {
                syllables.push(part.to_string());
            }
        }

        let cost = syllables.len() as u32;
        vec![SegmentPath {
            syllables,
            remaining,
            cost,
        }]
    }

    /// 模糊音等价性判定
    fn is_fuzzy_valid(slice: &str, fuzzy: &FuzzyConfig) -> bool {
        if fuzzy.z_zh {
            let alt = if let Some(rest) = slice.strip_prefix("zh") {
                format!("z{}", rest)
            } else if let Some(rest) = slice.strip_prefix('z') {
                format!("zh{}", rest)
            } else {
                String::new()
            };
            if !alt.is_empty() && is_valid_syllable(&alt) {
                return true;
            }
        }
        if fuzzy.c_ch {
            let alt = if let Some(rest) = slice.strip_prefix("ch") {
                format!("c{}", rest)
            } else if let Some(rest) = slice.strip_prefix('c') {
                format!("ch{}", rest)
            } else {
                String::new()
            };
            if !alt.is_empty() && is_valid_syllable(&alt) {
                return true;
            }
        }
        if fuzzy.s_sh {
            let alt = if let Some(rest) = slice.strip_prefix("sh") {
                format!("s{}", rest)
            } else if let Some(rest) = slice.strip_prefix('s') {
                format!("sh{}", rest)
            } else {
                String::new()
            };
            if !alt.is_empty() && is_valid_syllable(&alt) {
                return true;
            }
        }
        if fuzzy.in_ing {
            if let Some(rest) = slice.strip_suffix("ing") {
                let alt = format!("{}in", rest);
                if is_valid_syllable(&alt) {
                    return true;
                }
            } else if let Some(rest) = slice.strip_suffix("in") {
                let alt = format!("{}ing", rest);
                if is_valid_syllable(&alt) {
                    return true;
                }
            }
        }
        false
    }

    /// 双拼键位解码（支持小鹤双拼与自然码双拼）
    fn decode_double_pinyin(input: &str, schema: DoublePinyinSchema) -> Vec<SegmentPath> {
        let mut syllables = Vec::new();
        let chars: Vec<char> = input.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            if i + 1 < chars.len() {
                let initial = chars[i];
                let final_char = chars[i + 1];
                let decoded = match schema {
                    DoublePinyinSchema::XiaoHe => Self::map_xiaohe_pair(initial, final_char),
                    DoublePinyinSchema::ZiRanMa => Self::map_ziranma_pair(initial, final_char),
                };
                syllables.push(decoded);
                i += 2;
            } else {
                let single = chars[i].to_string();
                return vec![SegmentPath {
                    syllables,
                    remaining: single,
                    cost: 1,
                }];
            }
        }

        vec![SegmentPath {
            syllables,
            remaining: String::new(),
            cost: 1,
        }]
    }

    /// 小鹤双拼声韵母映射
    fn map_xiaohe_pair(initial: char, final_char: char) -> String {
        // 声母映射
        let init_str = match initial {
            'v' => "zh",
            'i' => "ch",
            'u' => "sh",
            c => return format!("{}{}", c, Self::map_xiaohe_final(final_char)),
        };
        format!("{}{}", init_str, Self::map_xiaohe_final(final_char))
    }

    /// 小鹤双拼韵母映射
    fn map_xiaohe_final(f: char) -> &'static str {
        match f {
            'q' => "iu",
            'w' => "ei",
            'e' => "e",
            'r' => "uan",
            't' => "ue",
            'y' => "un",
            'u' => "u",
            'i' => "i",
            'o' => "uo",
            'p' => "ie",
            'a' => "a",
            's' => "ong",
            'd' => "ai",
            'f' => "en",
            'g' => "eng",
            'h' => "ang",
            'j' => "an",
            'k' => "uai",
            'l' => "iang",
            'z' => "ou",
            'x' => "ia",
            'c' => "ao",
            'v' => "ui",
            'b' => "in",
            'n' => "iao",
            'm' => "ian",
            _ => "a",
        }
    }

    /// 自然码双拼声韵母映射
    fn map_ziranma_pair(initial: char, final_char: char) -> String {
        let init_str = match initial {
            'v' => "zh",
            'i' => "ch",
            'u' => "sh",
            c => return format!("{}{}", c, Self::map_ziranma_final(c, final_char)),
        };
        format!("{}{}", init_str, Self::map_ziranma_final(initial, final_char))
    }

    /// 自然码双拼韵母映射
    fn map_ziranma_final(initial: char, f: char) -> &'static str {
        match f {
            'q' => "iu",
            'w' => {
                if matches!(initial, 'j' | 'q' | 'x' | 'l' | 'd') {
                    "ia"
                } else {
                    "ua"
                }
            }
            'e' => "e",
            'r' => "uan",
            't' => "ue",
            'y' => "ing",
            'u' => "u",
            'i' => "i",
            'o' => "uo",
            'p' => "un",
            'a' => "a",
            's' => {
                if matches!(initial, 'j' | 'q' | 'x') {
                    "iong"
                } else {
                    "ong"
                }
            }
            'd' => "ai",
            'f' => "en",
            'g' => "eng",
            'h' => "ang",
            'j' => "an",
            'k' => "ao",
            'l' => {
                if matches!(initial, 'j' | 'q' | 'x' | 'l' | 'n') {
                    "iang"
                } else {
                    "uang"
                }
            }
            'z' => "ou",
            'x' => {
                if matches!(initial, 'j' | 'q' | 'x') {
                    "ia"
                } else {
                    "ua"
                }
            }
            'c' => "ao",
            'v' => "ui",
            'b' => "in",
            'n' => "in",
            'm' => "ian",
            _ => "a",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_ambiguity_xian() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("xian", &fuzzy, None);
        let key_strings: Vec<String> = paths.iter().map(|p| p.to_key_string()).collect();

        // 必须同时包含单音节 xian 与双音节 xi'an
        assert!(key_strings.contains(&"xian".to_string()));
        assert!(key_strings.contains(&"xi'an".to_string()));
    }

    #[test]
    fn test_explicit_split_quote() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("xi'an", &fuzzy, None);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].syllables, vec!["xi", "an"]);
    }

    #[test]
    fn test_partial_remaining() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("nih", &fuzzy, None);
        assert!(!paths.is_empty());
        assert_eq!(paths[0].syllables, vec!["ni"]);
        assert_eq!(paths[0].remaining, "h");
    }

    #[test]
    fn test_xiaohe_double_pinyin() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("xm", &fuzzy, Some(DoublePinyinSchema::XiaoHe));
        assert_eq!(paths[0].syllables, vec!["xian"]);
    }

    #[test]
    fn test_ziranma_double_pinyin() {
        let fuzzy = FuzzyConfig::default();
        let paths = PrismEngine::build_paths("xm", &fuzzy, Some(DoublePinyinSchema::ZiRanMa));
        assert_eq!(paths[0].syllables, vec!["xian"]);
    }

    #[test]
    fn test_strict_raw_initials_segmentation() {
        let fuzzy = FuzzyConfig::default();
        // 用户输入纯声母长串 qwdwwqswsqw，测试切分是否 100% 原按键保真，绝不篡改任何字母
        let paths = PrismEngine::build_paths("qwdwwqswsqw", &fuzzy, None);
        assert!(!paths.is_empty(), "切分路径不应为空");
        let best_path = &paths[0];
        assert_eq!(
            best_path.to_key_string(),
            "q'w'd'w'w'q's'w's'q'w",
            "声母切分音节必须严格保持用户输入的原始字母，绝不允许换字母"
        );
    }
}
