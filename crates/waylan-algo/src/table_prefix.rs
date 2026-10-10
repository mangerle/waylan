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
}
