use crate::prism::SegmentPath;
use crate::syllables::is_valid_syllable;
use crate::types::FuzzyConfig;

/// 拼音模糊音规则展开与等价键扩展模块
pub struct PrismFuzzy;
impl PrismFuzzy {
    pub fn expand_fuzzy_syllables(slice: &str, fuzzy: &FuzzyConfig) -> Vec<String> {
        if !fuzzy.has_active_rules() || slice.is_empty() {
            return Vec::new();
        }

        // 1. 收集声母等价变体（含原切片自身）
        let mut init_variants = Vec::with_capacity(4);
        init_variants.push(slice.to_string());

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

        // 2. 在声母变体基础上叠加韵母等价变体（仅按需分配变体字符串，避免无谓克隆）
        let mut all_variants = Vec::with_capacity(4);
        let mut final_cands = Vec::with_capacity(3);
        for v in init_variants {
            final_cands.clear();
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
            let mut syl_options = Vec::with_capacity(4);
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

            if syl_options.len() == 1 {
                let single_opt = &syl_options[0];
                for prefix in &mut combos {
                    prefix.push(single_opt.clone());
                }
                continue;
            }

            let mut next_combos = Vec::with_capacity(16);
            for prefix in &combos {
                for opt in &syl_options {
                    if next_combos.len() >= 16 {
                        break;
                    }
                    let mut next = Vec::with_capacity(syllables.len());
                    next.extend_from_slice(prefix);
                    next.push(opt.clone());
                    next_combos.push(next);
                }
                if next_combos.len() >= 16 {
                    break;
                }
            }
            combos = next_combos;
        }

        combos
            .into_iter()
            .filter(|parts| parts != syllables)
            .collect()
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

    /// 模糊音等价性判定
    pub fn is_fuzzy_valid(slice: &str, fuzzy: &FuzzyConfig) -> bool {
        !Self::expand_fuzzy_syllables(slice, fuzzy).is_empty()
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
