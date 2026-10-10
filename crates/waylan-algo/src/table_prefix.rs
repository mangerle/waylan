use crate::syllables::STANDARD_SYLLABLES;
use crate::table::LexiconTable;
use crate::types::{AlgoCandidate, CandidateTier};

impl LexiconTable {
    /// 检索带末尾残存拼音前缀的补全候选词列表
    pub fn lookup_prefix_extension(
        &self,
        prefix_syllables: &[String],
        remaining_initial: &str,
    ) -> Vec<AlgoCandidate> {
        if remaining_initial.is_empty() {
            return Vec::new();
        }

        let mut candidates = Vec::new();

        if prefix_syllables.is_empty() {
            for &syl in STANDARD_SYLLABLES {
                if syl.starts_with(remaining_initial) {
                    if let Some(matches) = self.get_raw_entries(syl) {
                        for (text, score) in matches {
                            if text.chars().count() == 1 {
                                candidates.push(
                                    AlgoCandidate::new(text, syl, score)
                                        .with_tier(CandidateTier::ExactWord),
                                );
                            }
                        }
                    }
                }
            }
        } else {
            let prefix_key = prefix_syllables.join("'");
            for &syl in STANDARD_SYLLABLES {
                if syl.starts_with(remaining_initial) {
                    let full_key = format!("{}'{}", prefix_key, syl);
                    if let Some(matches) = self.get_raw_entries(&full_key) {
                        for (text, score) in matches {
                            let char_len = text.chars().count() as u32;
                            let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                            let final_score = score.saturating_add(boost);
                            candidates.push(
                                AlgoCandidate::new(text, full_key.clone(), final_score)
                                    .with_tier(CandidateTier::ExactWord),
                            );
                        }
                    }
                }
            }
        }

        candidates.sort_by_key(|c| std::cmp::Reverse(c.score));
        candidates.truncate(20);
        candidates
    }

    /// 检索指定单字母声母对应的高频单字原始词库条目列表
    pub fn get_initial_raw_entries(&self, initial: &str) -> Option<Vec<(String, u32)>> {
        if let Some(compact) = &self.compact_mmap {
            if let Some(entries) = compact.lookup_initial(initial) {
                return Some(entries);
            }
        }
        self.initial_entries.get(initial).cloned()
    }

    /// 检索首字母缩写简拼候选词（如 "bj" 匹配 "北京"）
    pub fn lookup_acronym(&self, key: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();

        if let Some(compact) = &self.compact_mmap {
            if let Some(matches) = compact.lookup_acronym(key) {
                for (text, score) in matches {
                    candidates.push(
                        AlgoCandidate::new(text, key, score)
                            .with_tier(CandidateTier::Acronym)
                            .with_comment("[简拼]"),
                    );
                }
            }
        }

        if let Some(matches) = self.acronym_entries.get(key) {
            candidates.reserve(matches.len());
            for (text, score) in matches {
                candidates.push(
                    AlgoCandidate::new(text.clone(), key, *score)
                        .with_tier(CandidateTier::Acronym)
                        .with_comment("[简拼]"),
                );
            }
        }

        candidates
    }
}
