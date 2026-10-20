use crate::prism::SegmentPath;
use crate::syllables::is_valid_syllable;
use crate::types::FuzzyConfig;

/// 模糊音成对变换规则结构
type RulePred = fn(&FuzzyConfig) -> bool;

struct FuzzyPair {
    a: &'static str,
    b: &'static str,
    is_active: RulePred,
}

const INITIAL_RULES: &[FuzzyPair] = &[
    FuzzyPair {
        a: "zh",
        b: "z",
        is_active: |c| c.z_zh,
    },
    FuzzyPair {
        a: "ch",
        b: "c",
        is_active: |c| c.c_ch,
    },
    FuzzyPair {
        a: "sh",
        b: "s",
        is_active: |c| c.s_sh,
    },
    FuzzyPair {
        a: "l",
        b: "n",
        is_active: |c| c.l_n,
    },
    FuzzyPair {
        a: "f",
        b: "h",
        is_active: |c| c.f_h,
    },
    FuzzyPair {
        a: "r",
        b: "l",
        is_active: |c| c.r_l,
    },
    FuzzyPair {
        a: "g",
        b: "k",
        is_active: |c| c.g_k,
    },
];

const FINAL_RULES: &[FuzzyPair] = &[
    FuzzyPair {
        a: "ang",
        b: "an",
        is_active: |c| c.an_ang,
    },
    FuzzyPair {
        a: "eng",
        b: "en",
        is_active: |c| c.en_eng,
    },
    FuzzyPair {
        a: "ing",
        b: "in",
        is_active: |c| c.in_ing,
    },
];

/// 尝试基于成对前缀规则查找替换变体
#[inline]
fn match_prefix_pair<'a>(text: &'a str, rule: &FuzzyPair) -> Option<(&'static str, &'a str)> {
    if let Some(rest) = text.strip_prefix(rule.a) {
        Some((rule.b, rest))
    } else if let Some(rest) = text.strip_prefix(rule.b) {
        Some((rule.a, rest))
    } else {
        None
    }
}

/// 尝试基于成对后缀规则查找替换变体
#[inline]
fn match_suffix_pair<'a>(text: &'a str, rule: &FuzzyPair) -> Option<(&'a str, &'static str)> {
    if let Some(stem) = text.strip_suffix(rule.a) {
        Some((stem, rule.b))
    } else if let Some(stem) = text.strip_suffix(rule.b) {
        Some((stem, rule.a))
    } else {
        None
    }
}

/// 在栈上拼接声母、词干与韵母并校验是否为不同于原切片的有效拼音音节
#[inline]
fn check_stack(prefix: &str, stem: &str, suffix: &str, original: &str) -> bool {
    let mut buf = [0u8; 16];
    let p_bytes = prefix.as_bytes();
    let st_bytes = stem.as_bytes();
    let sf_bytes = suffix.as_bytes();
    let total = p_bytes.len() + st_bytes.len() + sf_bytes.len();
    if total > buf.len() {
        return false;
    }
    buf[..p_bytes.len()].copy_from_slice(p_bytes);
    buf[p_bytes.len()..p_bytes.len() + st_bytes.len()].copy_from_slice(st_bytes);
    buf[p_bytes.len() + st_bytes.len()..total].copy_from_slice(sf_bytes);
    if let Ok(s) = std::str::from_utf8(&buf[..total]) {
        s != original && is_valid_syllable(s)
    } else {
        false
    }
}

/// 检查在指定声母和主体下，叠加韵母规则是否能构成不同于原切片的合法音节
#[inline]
fn check_fuzzy_finals(init: &str, rest: &str, slice: &str, fuzzy: &FuzzyConfig) -> bool {
    for rule in FINAL_RULES {
        if (rule.is_active)(fuzzy) {
            if let Some((stem, target_final)) = match_suffix_pair(rest, rule) {
                if check_stack(init, stem, target_final, slice) {
                    return true;
                }
            }
        }
    }
    false
}

/// 拼音模糊音规则展开与等价键扩展模块
pub struct PrismFuzzy;
impl PrismFuzzy {
    /// 按用户启用的模糊音规则展开单个拼音切片的全部等价变体
    ///
    /// 未启用任何模糊规则或切片为空时返回空集合，调用方可据此跳过模糊搜索分支。
    pub fn expand_fuzzy_syllables(slice: &str, fuzzy: &FuzzyConfig) -> Vec<String> {
        if !fuzzy.has_active_rules() || slice.is_empty() {
            return Vec::new();
        }

        // 1. 收集声母等价变体（含原切片自身）
        let mut init_variants = Vec::with_capacity(4);
        init_variants.push(slice.to_string());

        for rule in INITIAL_RULES {
            if (rule.is_active)(fuzzy) {
                if let Some((target_init, rest)) = match_prefix_pair(slice, rule) {
                    init_variants.push(format!("{}{}", target_init, rest));
                }
            }
        }

        // 2. 在声母变体基础上叠加韵母等价变体（仅按需分配变体字符串，避免无谓克隆）
        let mut all_variants = Vec::with_capacity(4);
        let mut final_cands = Vec::with_capacity(3);
        for v in init_variants {
            final_cands.clear();
            for rule in FINAL_RULES {
                if (rule.is_active)(fuzzy) {
                    if let Some((stem, target_final)) = match_suffix_pair(&v, rule) {
                        final_cands.push(format!("{}{}", stem, target_final));
                    }
                }
            }

            if v != slice && is_valid_syllable(&v) && !all_variants.contains(&v) {
                all_variants.push(v);
            }
            for cand in final_cands.drain(..) {
                if cand != slice && is_valid_syllable(&cand) && !all_variants.contains(&cand) {
                    all_variants.push(cand);
                }
            }
        }

        all_variants
    }

    /// 展开音节序列在当前模糊音配置下的所有等价音节组合（不含原始音节序列自身）
    pub fn expand_fuzzy_syllable_lists(
        syllables: &[String],
        fuzzy: &FuzzyConfig,
    ) -> Vec<Vec<String>> {
        if !fuzzy.has_active_rules() || syllables.is_empty() {
            return Vec::new();
        }

        let mut combos: Vec<Vec<String>> = Vec::with_capacity(16);
        combos.push(Vec::with_capacity(syllables.len()));
        for syl in syllables {
            let syl_options = Self::collect_syllable_options(syl, fuzzy);

            if syl_options.len() == 1 {
                let single_opt = &syl_options[0];
                for prefix in &mut combos {
                    prefix.push(single_opt.clone());
                }
                continue;
            }

            combos = Self::expand_combos(combos, &syl_options, syllables.len());
        }

        combos
            .into_iter()
            .filter(|parts| parts != syllables)
            .collect()
    }

    /// 收集单个音节在当前模糊音配置下的全部等价候选
    ///
    /// 原始音节优先入选；若模糊音扩展未产出任何有效候选，则回退为原始音节本身。
    fn collect_syllable_options(syl: &str, fuzzy: &FuzzyConfig) -> Vec<String> {
        let mut syl_options = Vec::with_capacity(4);
        if is_valid_syllable(syl) {
            syl_options.push(syl.to_string());
        }
        for alt in Self::expand_fuzzy_syllables(syl, fuzzy) {
            if !syl_options.contains(&alt) {
                syl_options.push(alt);
            }
        }
        if syl_options.is_empty() {
            syl_options.push(syl.to_string());
        }
        syl_options
    }

    /// 将当前音节的候选组合与已有前缀组合做笛卡尔积展开（限制最大组合数以控制爆炸）
    fn expand_combos(
        combos: Vec<Vec<String>>,
        syl_options: &[String],
        syl_count: usize,
    ) -> Vec<Vec<String>> {
        let mut next_combos = Vec::with_capacity(16);
        for prefix in &combos {
            for opt in syl_options {
                if next_combos.len() >= 16 {
                    break;
                }
                let mut next = Vec::with_capacity(syl_count);
                next.extend_from_slice(prefix);
                next.push(opt.clone());
                next_combos.push(next);
            }
            if next_combos.len() >= 16 {
                break;
            }
        }
        next_combos
    }

    /// 展开一条切分路径在当前模糊音配置下的所有等价词典查询键（不含原始 `path.to_key_string()`）
    pub fn expand_fuzzy_keys(path: &SegmentPath, fuzzy: &FuzzyConfig) -> Vec<String> {
        if !fuzzy.has_active_rules() || path.syllables.is_empty() || !path.remaining.is_empty() {
            return Vec::new();
        }

        Self::expand_fuzzy_syllable_lists(&path.syllables, fuzzy)
            .into_iter()
            .map(|parts| parts.join("'"))
            .collect()
    }

    /// 模糊音等价性判定（快速短路，零堆分配）
    pub fn is_fuzzy_valid(slice: &str, fuzzy: &FuzzyConfig) -> bool {
        if !fuzzy.has_active_rules() || slice.is_empty() || slice.len() > 7 || !slice.is_ascii() {
            return false;
        }

        // 1. 检查原始切片结合韵母模糊音规则
        if check_fuzzy_finals("", slice, slice, fuzzy) {
            return true;
        }

        // 2. 检查声母模糊音变换及其韵母组合
        for rule in INITIAL_RULES {
            if (rule.is_active)(fuzzy) {
                if let Some((target_init, rest)) = match_prefix_pair(slice, rule) {
                    if check_stack(target_init, rest, "", slice) {
                        return true;
                    }
                    if check_fuzzy_finals(target_init, rest, slice, fuzzy) {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// 零声母与特殊单辅音音节集合（以 a, o, e 开头，或特殊叹词音节 n, ng, m）
    #[inline]
    pub fn is_zero_initial_syllable(syl: &str) -> bool {
        matches!(
            syl,
            "a" | "ai"
                | "an"
                | "ang"
                | "ao"
                | "e"
                | "ei"
                | "en"
                | "eng"
                | "er"
                | "o"
                | "ou"
                | "n"
                | "ng"
                | "m"
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
    fn test_initial_rules_expansion_and_validity() {
        let fuzzy = FuzzyConfig {
            z_zh: true,
            c_ch: true,
            s_sh: true,
            l_n: true,
            f_h: true,
            r_l: true,
            g_k: true,
            ..Default::default()
        };

        assert!(PrismFuzzy::is_fuzzy_valid("zhi", &fuzzy));
        assert!(PrismFuzzy::is_fuzzy_valid("zi", &fuzzy));
        let alts = PrismFuzzy::expand_fuzzy_syllables("zhi", &fuzzy);
        assert!(alts.contains(&"zi".to_string()));

        let alts = PrismFuzzy::expand_fuzzy_syllables("chi", &fuzzy);
        assert!(alts.contains(&"ci".to_string()));

        let alts = PrismFuzzy::expand_fuzzy_syllables("shi", &fuzzy);
        assert!(alts.contains(&"si".to_string()));

        let alts = PrismFuzzy::expand_fuzzy_syllables("li", &fuzzy);
        assert!(alts.contains(&"ni".to_string()));

        let alts = PrismFuzzy::expand_fuzzy_syllables("hu", &fuzzy);
        assert!(alts.contains(&"fu".to_string()));

        let alts = PrismFuzzy::expand_fuzzy_syllables("re", &fuzzy);
        assert!(alts.contains(&"le".to_string()));

        let alts = PrismFuzzy::expand_fuzzy_syllables("ge", &fuzzy);
        assert!(alts.contains(&"ke".to_string()));
    }

    #[test]
    fn test_final_rules_expansion_and_combination() {
        let fuzzy = FuzzyConfig {
            z_zh: true,
            an_ang: true,
            en_eng: true,
            in_ing: true,
            ..Default::default()
        };

        let alts = PrismFuzzy::expand_fuzzy_syllables("zhang", &fuzzy);
        assert!(alts.contains(&"zang".to_string()));
        assert!(alts.contains(&"zhan".to_string()));
        assert!(alts.contains(&"zan".to_string()));

        assert!(PrismFuzzy::is_fuzzy_valid("zhang", &fuzzy));
        assert!(PrismFuzzy::is_fuzzy_valid("pin", &fuzzy));
        let alts = PrismFuzzy::expand_fuzzy_syllables("pin", &fuzzy);
        assert!(alts.contains(&"ping".to_string()));
    }

    #[test]
    fn test_zero_initial_and_borrowing_split() {
        assert!(PrismFuzzy::is_zero_initial_syllable("an"));
        assert!(PrismFuzzy::is_zero_initial_syllable("er"));
        assert!(!PrismFuzzy::is_zero_initial_syllable("ba"));

        // "ti" + "an" => "tian" (合法)
        assert!(PrismFuzzy::is_zero_initial_borrowing_split("ti", "an"));
        // "ba" + "ba" (ba 不是零声母)
        assert!(!PrismFuzzy::is_zero_initial_borrowing_split("ba", "ba"));
    }
}
