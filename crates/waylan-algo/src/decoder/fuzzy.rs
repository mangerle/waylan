//! 拼音模糊音规则展开与等价性校验模块
//!
//! # 设计原理
//! - **实现初衷**：根据用户启用的九大模糊音规则，在统一音节格构建阶段将单个标准音节扩展为等价变体。
//! - **核心优势**：零栈外分配快速校验 `is_fuzzy_valid`，并在 `expand_fuzzy_syllables` 中支持声母+韵母叠加扩展。
//! - **代价与局限**：仅扩展单音节变体，具体词组组合由后续 DAT 游标和束搜索统一竞争完成。

use crate::syllables::is_valid_syllable;
use crate::types::FuzzyConfig;

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

/// 拼音模糊音规则展开与等价判定器
pub struct PrismFuzzy;

impl PrismFuzzy {
    /// 按用户启用的模糊音规则展开单个拼音切片的全部等价变体（不含原切片自身）
    pub fn expand_fuzzy_syllables(slice: &str, fuzzy: &FuzzyConfig) -> Vec<String> {
        if !fuzzy.has_active_rules() || slice.is_empty() {
            return Vec::new();
        }

        let mut init_variants = Vec::with_capacity(4);
        init_variants.push(slice.to_string());

        for rule in INITIAL_RULES {
            if (rule.is_active)(fuzzy) {
                if let Some((target_init, rest)) = match_prefix_pair(slice, rule) {
                    init_variants.push(format!("{target_init}{rest}"));
                }
            }
        }

        let mut all_variants = Vec::with_capacity(4);
        let mut final_cands = Vec::with_capacity(3);
        for v in init_variants {
            final_cands.clear();
            for rule in FINAL_RULES {
                if (rule.is_active)(fuzzy) {
                    if let Some((stem, target_final)) = match_suffix_pair(&v, rule) {
                        final_cands.push(format!("{stem}{target_final}"));
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

    /// 模糊音等价性判定（快速短路，零堆分配）
    pub fn is_fuzzy_valid(slice: &str, fuzzy: &FuzzyConfig) -> bool {
        if !fuzzy.has_active_rules() || slice.is_empty() || slice.len() > 7 || !slice.is_ascii() {
            return false;
        }

        if check_fuzzy_finals("", slice, slice, fuzzy) {
            return true;
        }

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
