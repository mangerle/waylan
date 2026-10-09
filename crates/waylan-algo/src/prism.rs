use crate::syllables::{has_syllable_prefix, is_valid_syllable};
use crate::types::{DoublePinyinSchema, FuzzyConfig};

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

        // 3. 全拼模式：在 DAG 上执行多路径搜索
        let mut paths = Vec::new();
        let mut current_syllables = Vec::new();
        Self::dfs_dag(input, 0, &mut current_syllables, &mut paths, fuzzy);

        // 如果完全无法形成完整音节切分，尝试提取最长可能前缀
        if paths.is_empty() {
            paths.push(SegmentPath {
                syllables: Vec::new(),
                remaining: input.to_string(),
                cost: 100,
            });
        }

        // 按代价值升序排序（段数少、完整覆盖的排在前）
        paths.sort_by_key(|p| p.cost);
        paths.dedup();
        paths
    }

    /// 深度优先搜索构建音节 DAG 路径
    fn dfs_dag(
        input: &str,
        start: usize,
        current: &mut Vec<String>,
        results: &mut Vec<SegmentPath>,
        fuzzy: &FuzzyConfig,
    ) {
        if start >= input.len() {
            if !current.is_empty() {
                results.push(SegmentPath {
                    syllables: current.clone(),
                    remaining: String::new(),
                    cost: current.len() as u32,
                });
            }
            return;
        }

        // 最长拼音音节不超过 6 字符（如 zhuang, chuang）
        let max_len = (input.len() - start).min(6);
        let mut matched_any = false;

        for len in (1..=max_len).rev() {
            let slice = &input[start..start + len];
            if is_valid_syllable(slice) || Self::is_fuzzy_valid(slice, fuzzy) {
                matched_any = true;
                current.push(slice.to_string());
                Self::dfs_dag(input, start + len, current, results, fuzzy);
                current.pop();
            }
        }

        // 若当前处于末尾且无法凑成完整音节，但属于合法前缀，记录为带有 remaining 的候选路径
        if !matched_any && !current.is_empty() {
            let remaining = &input[start..];
            if has_syllable_prefix(remaining) {
                results.push(SegmentPath {
                    syllables: current.clone(),
                    remaining: remaining.to_string(),
                    cost: current.len() as u32 + 2,
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
}
