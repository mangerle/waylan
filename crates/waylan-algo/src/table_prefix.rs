use crate::syllables::find_syllables_by_prefix;
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

        let matched_syllables = find_syllables_by_prefix(remaining_initial);
        let mut candidates = Vec::with_capacity(matched_syllables.len() * 4);

        if prefix_syllables.is_empty() {
            for &syl in matched_syllables {
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
        } else {
            let prefix_key = prefix_syllables.join("'");
            let mut full_key = String::with_capacity(prefix_key.len() + 10);
            for &syl in matched_syllables {
                full_key.clear();
                full_key.push_str(&prefix_key);
                full_key.push('\'');
                full_key.push_str(syl);
                if let Some(matches) = self.get_raw_entries(&full_key) {
                    for (text, score) in matches {
                        let char_len = text.chars().count() as u32;
                        let boost = char_len.saturating_sub(1).saturating_mul(600_000);
                        let final_score = score.saturating_add(boost);
                        candidates.push(
                            AlgoCandidate::new(text, full_key.as_str(), final_score)
                                .with_tier(CandidateTier::ExactWord),
                        );
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
        let mut candidates = Vec::with_capacity(16);

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

        candidates.sort_by_key(|c| std::cmp::Reverse(c.score));
        candidates
    }
}
