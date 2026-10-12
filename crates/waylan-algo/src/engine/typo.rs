use crate::prism::PrismEngine;
use crate::syllables;
use crate::table::LexiconTable;
use crate::types::{self, AlgoCandidate, CandidateTier};

/// 物理键盘空间邻键误触与相邻字符倒置容错纠错模块
pub struct SpatialTypoCorrector;

impl SpatialTypoCorrector {
    /// 生成基于物理键盘邻键误触与相邻倒置的次位容错候选
    ///
    /// # 设计原理
    /// - **空间距离判定**：基于 QWERTY 键盘欧几里得距离，限制在 1.25 键距内的邻键替换。
    /// - **相邻字母倒置**：针对拼音快速击键常见的前后顺序倒置（如 `teh` -> `the`, `cehng` -> `cheng`）。
    /// - **排序与截断**：按替换物理代价升序、词频降序综合排序，仅返回前 3 个最优纠错结果。
    pub fn generate_candidates(raw_input: &str, table: &LexiconTable) -> Vec<AlgoCandidate> {
        let n = raw_input.len();
        // 限制拼写纠错仅在常用词长（3..=8）内启用，避免长拼音下触发数百次全量 DAG 搜索导致击键延迟退化
        if !(3..=8).contains(&n) || !raw_input.is_ascii() {
            return Vec::new();
        }

        let mut candidates: Vec<(AlgoCandidate, u32)> = Vec::with_capacity(8);
        let mut buf = [0u8; 24];
        buf[..n].copy_from_slice(raw_input.as_bytes());

        // 1. 单字符邻键空间替换
        for i in 0..n {
            let orig_b = buf[i];
            if !orig_b.is_ascii_lowercase() {
                continue;
            }
            let orig_c = orig_b as char;

            for alt_b in b'a'..=b'z' {
                if alt_b == orig_b {
                    continue;
                }
                let alt_c = alt_b as char;
                let dist = syllables::key_spatial_distance(orig_c, alt_c);
                if dist > 1.25 {
                    continue;
                }

                buf[i] = alt_b;
                if let Ok(mutated) = std::str::from_utf8(&buf[..n]) {
                    // 轻量快速路径：若变异后本身构成单个合法音节，直接查询词库无需跑完整 DAG
                    if syllables::is_valid_syllable(mutated) {
                        if let Some(entries) = table.get_raw_entries(mutated) {
                            let cost = syllables::char_substitution_cost(orig_c, alt_c);
                            for (text, raw_score) in entries.iter().take(2) {
                                let mut cand = AlgoCandidate::new(text, mutated, *raw_score);
                                cand.tier = CandidateTier::TypoCorrection;
                                cand.comment = Some(format!("[纠错: {}]", mutated));
                                candidates.push((cand, cost));
                            }
                        }
                    } else if syllables::can_segment_into_syllables(mutated) {
                        let paths =
                            PrismEngine::build_paths(mutated, &types::FuzzyConfig::default());
                        if let Some(best) = paths.first() {
                            if best.remaining.is_empty() && !best.syllables.is_empty() {
                                let key = best.to_key_string();
                                if let Some(entries) = table.get_raw_entries(&key) {
                                    let cost = syllables::char_substitution_cost(orig_c, alt_c);
                                    for (text, raw_score) in entries.iter().take(2) {
                                        let mut cand = AlgoCandidate::new(text, &key, *raw_score);
                                        cand.tier = CandidateTier::TypoCorrection;
                                        cand.comment = Some(format!("[纠错: {}]", key));
                                        candidates.push((cand, cost));
                                    }
                                }
                            }
                        }
                    }
                }
                buf[i] = orig_b;
            }
        }

        // 2. 邻近字母倒置
        for i in 0..n.saturating_sub(1) {
            buf.swap(i, i + 1);
            if let Ok(swapped) = std::str::from_utf8(&buf[..n]) {
                if syllables::is_valid_syllable(swapped) {
                    if let Some(entries) = table.get_raw_entries(swapped) {
                        let cost = 16;
                        for (text, raw_score) in entries.iter().take(2) {
                            let mut cand = AlgoCandidate::new(text, swapped, *raw_score);
                            cand.tier = CandidateTier::TypoCorrection;
                            cand.comment = Some(format!("[纠错: {}]", swapped));
                            candidates.push((cand, cost));
                        }
                    }
                } else if syllables::can_segment_into_syllables(swapped) {
                    let paths = PrismEngine::build_paths(swapped, &types::FuzzyConfig::default());
                    if let Some(best) = paths.first() {
                        if best.remaining.is_empty() && !best.syllables.is_empty() {
                            let key = best.to_key_string();
                            if let Some(entries) = table.get_raw_entries(&key) {
                                let cost = 16;
                                for (text, raw_score) in entries.iter().take(2) {
                                    let mut cand = AlgoCandidate::new(text, &key, *raw_score);
                                    cand.tier = CandidateTier::TypoCorrection;
                                    cand.comment = Some(format!("[纠错: {}]", key));
                                    candidates.push((cand, cost));
                                }
                            }
                        }
                    }
                }
            }
            buf.swap(i, i + 1);
        }

        // 3. 排序与去重：优先代价低，次优先词频高
        candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.score.cmp(&a.0.score)));

        let mut deduped = Vec::with_capacity(3);
        let mut seen = std::collections::HashSet::with_capacity(8);
        for (cand, _) in candidates {
            if seen.insert(cand.text.clone()) {
                deduped.push(cand);
                if deduped.len() >= 3 {
                    break;
                }
            }
        }

        deduped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spatial_typo_corrector_pruning_and_recovery() {
        let mut table = LexiconTable::new();
        table.insert("ni'hao", "你好", 50_000);
        table.insert("cheng", "成", 40_000);

        // 邻键替换纠错：nihso (s->a) 召回 你好
        let cands_sub = SpatialTypoCorrector::generate_candidates("nihso", &table);
        assert!(!cands_sub.is_empty());
        assert_eq!(cands_sub[0].text, "你好");

        // 相邻倒置纠错：cehng (eh->he) 召回 成
        let cands_swap = SpatialTypoCorrector::generate_candidates("cehng", &table);
        assert!(!cands_swap.is_empty());
        assert_eq!(cands_swap[0].text, "成");
    }
}
