//! 拼音模糊音位掩码、等价类索引与非传递性校验模块
//!
//! # 设计原理
//! - **实现初衷**：通过声母链/韵母链与位掩码异或距离，在零重建的前提下支持任意模糊音组合，
//!   并从数学上杜绝多步传递性混淆（如同时开启 `l<->n` 与 `r<->l` 时，绝不允许 `n` 跨过 `l` 变成 `r`）。
//! - **核心优势**：预计算静态等价类表，运行时 `is_fuzzy_valid` 零堆分配，
//!   `fuzzy_mask_between` 单次位异或即可精确判定所需规则组合。
//! - **代价与局限**：仅扩展单音节变体（声母最多替换 1 步、韵母最多替换 1 步），
//!   具体词组组合由后续 DAT 游标和束搜索统一竞争完成。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::syllables::standard::STANDARD_SYLLABLES;
use crate::syllables::{STANDARD_INITIALS, is_valid_syllable};
use crate::types::FuzzyConfig;

/// 模糊音位掩码定义
pub mod fuzzy_mask {
    use crate::types::FuzzyConfig;

    /// z <=> zh
    pub const ZH: u32 = 1 << 0;
    /// c <=> ch
    pub const CH: u32 = 1 << 1;
    /// s <=> sh
    pub const SH: u32 = 1 << 2;
    /// l <=> n
    pub const NL: u32 = 1 << 3;
    /// r <=> l
    pub const RL: u32 = 1 << 4;
    /// f <=> h
    pub const FH: u32 = 1 << 5;
    /// g <=> k
    pub const GK: u32 = 1 << 6;
    /// an <=> ang（含 ian/iang、uan/uang）
    pub const AN: u32 = 1 << 7;
    /// en <=> eng
    pub const EN: u32 = 1 << 8;
    /// in <=> ing
    pub const IN: u32 = 1 << 9;

    /// 声母模糊音掩码集合
    pub const INITIAL_MASK: u32 = ZH | CH | SH | NL | RL | FH | GK;
    /// 韵母模糊音掩码集合
    pub const FINAL_MASK: u32 = AN | EN | IN;

    /// 将 `FuzzyConfig` 布尔配置转换为位掩码
    #[inline]
    pub fn from_config(cfg: &FuzzyConfig) -> u32 {
        let mut mask = 0u32;
        if cfg.z_zh {
            mask |= ZH;
        }
        if cfg.c_ch {
            mask |= CH;
        }
        if cfg.s_sh {
            mask |= SH;
        }
        if cfg.l_n {
            mask |= NL;
        }
        if cfg.r_l {
            mask |= RL;
        }
        if cfg.f_h {
            mask |= FH;
        }
        if cfg.g_k {
            mask |= GK;
        }
        if cfg.an_ang {
            mask |= AN;
        }
        if cfg.en_eng {
            mask |= EN;
        }
        if cfg.in_ing {
            mask |= IN;
        }
        mask
    }
}

/// 声母替换链（每步消耗一个独立掩码位，仅相邻项允许一步互换）
const INITIAL_CHAINS: &[&[(&str, u32)]] = &[
    &[("z", 0), ("zh", fuzzy_mask::ZH)],
    &[("c", 0), ("ch", fuzzy_mask::CH)],
    &[("s", 0), ("sh", fuzzy_mask::SH)],
    &[("n", 0), ("l", fuzzy_mask::NL), ("r", fuzzy_mask::RL)],
    &[("f", 0), ("h", fuzzy_mask::FH)],
    &[("g", 0), ("k", fuzzy_mask::GK)],
];

/// 韵母替换链
const FINAL_CHAINS: &[&[(&str, u32)]] = &[
    &[("an", 0), ("ang", fuzzy_mask::AN)],
    &[("ian", 0), ("iang", fuzzy_mask::AN)],
    &[("uan", 0), ("uang", fuzzy_mask::AN)],
    &[("en", 0), ("eng", fuzzy_mask::EN)],
    &[("in", 0), ("ing", fuzzy_mask::IN)],
];

struct SyllableFuzzyEntry {
    text: &'static str,
    pos_mask: u32,
}

struct FuzzyIndexTable {
    /// 每个等价类键对应的所有合法标准音节及其在等价类中的位置掩码
    classes: HashMap<String, Vec<SyllableFuzzyEntry>>,
}

/// 按声母表拆分音节（声母, 韵母）
#[inline]
fn split_initial_final(slice: &str) -> (&str, &str) {
    for &init in STANDARD_INITIALS {
        if let Some(rest) = slice.strip_prefix(init) {
            return (init, rest);
        }
    }
    ("", slice)
}

/// 计算局部音节成分在替换链中的等价头与累计位掩码
#[inline]
fn chain_head_and_mask<'a>(chains: &[&[(&'a str, u32)]], part: &'a str) -> (&'a str, u32) {
    for chain in chains {
        let mut mask = 0u32;
        for &(item, step_bit) in *chain {
            mask |= step_bit;
            if item == part {
                return (chain[0].0, mask);
            }
        }
    }
    (part, 0)
}

/// 计算任意拼音切片在全开模糊音下的（等价类键, 位置位掩码）
#[inline]
fn compute_class_and_mask(slice: &str) -> (String, u32) {
    let (init, final_part) = split_initial_final(slice);
    let (init_head, init_mask) = chain_head_and_mask(INITIAL_CHAINS, init);
    let (final_head, final_mask) = chain_head_and_mask(FINAL_CHAINS, final_part);
    let mut class_key = String::with_capacity(init_head.len() + 1 + final_head.len());
    class_key.push_str(init_head);
    class_key.push('|');
    class_key.push_str(final_head);
    (class_key, init_mask | final_mask)
}

/// 判定两个位置掩码之间是否满足“声母最多一步、韵母最多一步”的非传递性模糊约束，
/// 若合法则返回所需的模糊音掩码集合
#[inline]
fn diff_mask_if_valid(pos_a: u32, pos_b: u32) -> Option<u32> {
    let diff = pos_a ^ pos_b;
    let init_diff = diff & fuzzy_mask::INITIAL_MASK;
    let final_diff = diff & fuzzy_mask::FINAL_MASK;
    if init_diff.count_ones() <= 1 && final_diff.count_ones() <= 1 {
        Some(diff)
    } else {
        None
    }
}

fn fuzzy_index() -> &'static FuzzyIndexTable {
    static TABLE: OnceLock<FuzzyIndexTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut classes: HashMap<String, Vec<SyllableFuzzyEntry>> = HashMap::with_capacity(256);
        for &syl in STANDARD_SYLLABLES {
            let (class_key, pos_mask) = compute_class_and_mask(syl);
            classes
                .entry(class_key)
                .or_default()
                .push(SyllableFuzzyEntry {
                    text: syl,
                    pos_mask,
                });
        }
        FuzzyIndexTable { classes }
    })
}

/// 计算两个拼音音节之间模糊等价所需的规则位掩码（不满足非传递约束返回 `None`）
pub fn fuzzy_mask_between(a: &str, b: &str) -> Option<u32> {
    if a == b {
        return Some(0);
    }
    let (class_a, mask_a) = compute_class_and_mask(a);
    let (class_b, mask_b) = compute_class_and_mask(b);
    if class_a != class_b {
        return None;
    }
    diff_mask_if_valid(mask_a, mask_b)
}

/// 拼音模糊音规则展开与等价判定器
pub struct PrismFuzzy;

impl PrismFuzzy {
    /// 按用户启用的模糊音规则展开单个拼音切片的全部等价变体（不含原切片自身）
    pub fn expand_fuzzy_syllables(slice: &str, fuzzy: &FuzzyConfig) -> Vec<String> {
        let active_mask = fuzzy_mask::from_config(fuzzy);
        Self::expand_fuzzy_syllables_with_mask(slice, active_mask)
    }

    /// 按指定模糊音位掩码展开单个拼音切片的全部合法等价变体（不含原切片自身）
    pub fn expand_fuzzy_syllables_with_mask(slice: &str, active_mask: u32) -> Vec<String> {
        if active_mask == 0 || slice.is_empty() || slice.len() > 7 || !slice.is_ascii() {
            return Vec::new();
        }
        let (class_key, pos_mask) = compute_class_and_mask(slice);
        let Some(entries) = fuzzy_index().classes.get(&class_key) else {
            return Vec::new();
        };

        let mut variants = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.text == slice {
                continue;
            }
            if let Some(needed) = diff_mask_if_valid(pos_mask, entry.pos_mask)
                && needed != 0
                && (needed & active_mask) == needed
            {
                variants.push(entry.text.to_string());
            }
        }
        variants
    }

    /// 模糊音等价性判定（严格满足非传递性，快速短路）
    pub fn is_fuzzy_valid(slice: &str, fuzzy: &FuzzyConfig) -> bool {
        let active_mask = fuzzy_mask::from_config(fuzzy);
        if active_mask == 0 || slice.is_empty() || slice.len() > 7 || !slice.is_ascii() {
            return false;
        }
        let (class_key, pos_mask) = compute_class_and_mask(slice);
        let Some(entries) = fuzzy_index().classes.get(&class_key) else {
            return false;
        };

        entries.iter().any(|entry| {
            entry.text != slice
                && diff_mask_if_valid(pos_mask, entry.pos_mask)
                    .is_some_and(|needed| needed != 0 && (needed & active_mask) == needed)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_mask_prevents_transitive_confusion() {
        // 同时开启 l<->n 与 r<->l 时，n 不能跨过 l 变成 r
        let cfg = FuzzyConfig {
            l_n: true,
            r_l: true,
            ..FuzzyConfig::default()
        };

        let from_nan = PrismFuzzy::expand_fuzzy_syllables("nan", &cfg);
        assert!(from_nan.contains(&"lan".to_string()));
        assert!(!from_nan.contains(&"ran".to_string()));

        let from_ran = PrismFuzzy::expand_fuzzy_syllables("ran", &cfg);
        assert!(from_ran.contains(&"lan".to_string()));
        assert!(!from_ran.contains(&"nan".to_string()));

        let from_lan = PrismFuzzy::expand_fuzzy_syllables("lan", &cfg);
        assert!(from_lan.contains(&"nan".to_string()));
        assert!(from_lan.contains(&"ran".to_string()));

        assert_eq!(fuzzy_mask_between("nan", "ran"), None);
        assert_eq!(fuzzy_mask_between("nan", "lan"), Some(fuzzy_mask::NL));
        assert_eq!(fuzzy_mask_between("lan", "ran"), Some(fuzzy_mask::RL));
    }

    #[test]
    fn test_fuzzy_initial_and_final_combination() {
        let cfg = FuzzyConfig {
            z_zh: true,
            an_ang: true,
            ..FuzzyConfig::default()
        };

        let variants = PrismFuzzy::expand_fuzzy_syllables("zan", &cfg);
        assert!(variants.contains(&"zhan".to_string()));
        assert!(variants.contains(&"zang".to_string()));
        assert!(variants.contains(&"zhang".to_string()));
        assert!(PrismFuzzy::is_fuzzy_valid(
            "rai",
            &FuzzyConfig {
                r_l: true,
                ..FuzzyConfig::default()
            }
        ));
    }
}
