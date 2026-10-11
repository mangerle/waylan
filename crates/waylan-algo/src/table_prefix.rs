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
                if let Some(matches) = self.get_top_raw_entries(syl, 8) {
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
                if let Some(matches) = self.get_top_raw_entries(&full_key, 8) {
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
        let mut unique = Vec::with_capacity(candidates.len().min(20));
        for cand in candidates {
            if !unique.iter().any(|u: &AlgoCandidate| u.text == cand.text) {
                unique.push(cand);
                if unique.len() >= 20 {
                    break;
                }
            }
        }
        unique
    }

    /// 检索指定单字母声母对应的高频单字原始词库条目列表（合并紧凑词库与内存动态索引）
    pub fn get_initial_raw_entries(&self, initial: &str) -> Option<Vec<(String, u32)>> {
        let compact_res = self
            .compact_mmap
            .as_ref()
            .and_then(|c| c.lookup_initial(initial));
        let mem_res = self.initial_entries.get(initial);

        match (compact_res, mem_res) {
            (Some(mut c_list), Some(m_list)) => {
                for (text, score) in m_list {
                    Self::upsert_entry(&mut c_list, text.clone(), *score);
                }
                c_list.truncate(5);
                Some(c_list)
            }
            (Some(c_list), None) => Some(c_list),
            (None, Some(m_list)) => Some(m_list.clone()),
            (None, None) => None,
        }
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
