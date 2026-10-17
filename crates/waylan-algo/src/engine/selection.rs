use super::pangu::PanguSpacing;
use super::AlgoEngine;
use crate::dispatcher::ModeDispatcher;
use crate::types::{AlgoCandidate, CandidateTier};

/// 判断字符是否为 CJK 表意汉字（覆盖基本区、扩展区 A~I、兼容表意文字及汉字零“〇”）
#[inline]
pub(crate) fn is_cjk_ideograph(c: char) -> bool {
    matches!(
        c,
        '\u{3007}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2EBEF}'
            | '\u{2F800}'..='\u{2FA1F}'
            | '\u{30000}'..='\u{323AF}'
    )
}

impl AlgoEngine {
    /// 判断当前选中的候选词是否应当被记录到用户自适应词频模型中
    pub(crate) fn should_record_to_user_dict(&self, selected: &AlgoCandidate) -> bool {
        if selected.is_expandable || selected.pinyin.is_empty() || selected.text.is_empty() {
            return false;
        }
        if matches!(
            selected.tier,
            CandidateTier::TypoCorrection | CandidateTier::ProtectedEntity
        ) {
            return false;
        }
        if selected.comment.as_deref() == Some("自定义短语") {
            return false;
        }
        if ModeDispatcher::try_dispatch(self.composition.as_str(), &self.config).is_some() {
            return false;
        }
        selected.text.chars().all(is_cjk_ideograph)
    }

    pub fn select_candidate(
        &mut self,
        index: usize,
        current_timestamp: u64,
    ) -> Option<(String, usize)> {
        if index >= self.viewport.current_candidates.len() {
            return None;
        }
        let selected = self.viewport.current_candidates[index].clone();
        Some(self.commit_candidate_internal(selected, current_timestamp))
    }

    /// 从当前输入的全量候选词缓存中直接选取指定全局索引的词条上屏
    pub fn select_candidate_from_all(
        &mut self,
        global_index: usize,
        current_timestamp: u64,
    ) -> Option<(String, usize)> {
        if global_index >= self.viewport.all_candidates_cache.len() {
            return None;
        }
        let selected = self.viewport.all_candidates_cache[global_index].clone();
        Some(self.commit_candidate_internal(selected, current_timestamp))
    }

    /// 核心提交逻辑内部实现：自造词、调频、盘古之白与消费前缀
    fn commit_candidate_internal(
        &mut self,
        selected: AlgoCandidate,
        current_timestamp: u64,
    ) -> (String, usize) {
        self.adaptive.last_timestamp = self.adaptive.last_timestamp.max(current_timestamp);
        let should_record = self.should_record_to_user_dict(&selected);

        // 若选中的是可展开项，且直接按数字键确认（未展开时）：
        // 优先将首个格式化子候选文本（如当前标准日期 2026年10月9日）上屏
        let raw_commit = if selected.is_expandable && !selected.sub_candidates.is_empty() {
            selected.sub_candidates[0].clone()
        } else {
            selected.text.clone()
        };

        // 当选中的候选词为合规中文拼音词条（支持简繁中文）时，记录用户选词并动态调频
        if should_record {
            self.adaptive
                .record_commit(&selected.text, &selected.pinyin, current_timestamp);
        }

        // 盘古之白（中西文智能空格）：自动在交界处补充半角空格
        let commit_text = if self.config.auto_pangu_spacing {
            if let Some(prev) = self.composition.last_commit_context() {
                if PanguSpacing::should_insert_pangu_space(prev, &raw_commit) {
                    format!(" {}", raw_commit)
                } else {
                    raw_commit.clone()
                }
            } else {
                raw_commit.clone()
            }
        } else {
            raw_commit.clone()
        };

        // 记录上一次上屏词条作为跨句历史上下文（记录去除前后空格的纯净词条，避免干扰语言模型转移概率）
        let trimmed_commit = raw_commit.trim();
        if !trimmed_commit.is_empty() {
            self.composition
                .set_last_commit_context(Some(trimmed_commit.to_string()));
        }

        // 计算消费的拼音字母长度并执行分段确认（优先直接读取候选词产生时统一标记的 consumed_len，避免重复构图）
        let mut consumed_len = if let Some(len) = selected.consumed_len {
            len.min(self.composition.len())
        } else if selected.is_expandable
            || selected.tier == CandidateTier::TypoCorrection
            || self.composition.as_str().starts_with('v')
            || ModeDispatcher::try_dispatch(self.composition.as_str(), &self.config).is_some()
        {
            self.composition.len()
        } else {
            let calculated =
                Self::calculate_consumed_len(self.composition.as_str(), &selected.pinyin, &[]);
            if calculated > 0 {
                calculated
            } else {
                Self::compute_consumed_len(self.composition.as_str(), &selected.pinyin)
            }
        };

        // 核心安全保底：用户主动选词提交时，消费长度绝对不可为 0！
        // 若经过多级音节对齐计算后消费长度仍为 0，说明候选词属于整段联想或长词补全，
        // 必须保底消费全部剩余拼音，彻底消除状态机死锁与重复上屏。
        if consumed_len == 0 {
            log::warn!(
                "[算法引擎] 选词提交消费长度为 0，触发安全保底消费全部拼音: 词条=\"{}\", 拼音=\"{}\", 剩余拼音=\"{}\"",
                selected.text,
                selected.pinyin,
                self.composition.as_str()
            );
            consumed_len = self.composition.len();
        }

        if should_record {
            self.composition
                .record_partial_segment(selected.text.clone(), selected.pinyin.clone());
        } else {
            self.composition.clear_partial_segments();
        }

        self.composition.consume_prefix(consumed_len);

        // 若当前整串拼音已全部消费完毕，且通过 2 次及以上分段选词完成，自动合成为用户自造新词
        if self.composition.is_empty() {
            if self.composition.partial_segments().len() >= 2 {
                let full_text: String = self
                    .composition
                    .partial_segments()
                    .iter()
                    .map(|(t, _)| t.as_str())
                    .collect();
                let full_pinyin: String = self
                    .composition
                    .partial_segments()
                    .iter()
                    .map(|(_, p)| p.as_str())
                    .collect::<Vec<_>>()
                    .join("'");
                let char_len = full_text.chars().count();
                if (2..=8).contains(&char_len) {
                    self.adaptive
                        .record_commit(&full_text, &full_pinyin, current_timestamp);
                }
            }
            self.composition.clear_partial_segments();
        }

        self.viewport.page_index = 0;
        if self.composition.is_empty() {
            self.viewport.reset();
            self.composition.clear_cached_preedit();
        } else {
            self.recompute_candidates();
        }

        (commit_text, self.composition.len())
    }

    /// 切换二级扩展卡片的展开状态（若当前选中的项不可展开但存在可展开项，则自动聚焦并展开）
    pub fn toggle_expand(&mut self, selected_candidate_index: usize) -> bool {
        self.viewport.toggle_expand(selected_candidate_index)
    }

    /// 在二级展开卡片中向下移动选中项
    pub fn next_expanded_item(&mut self, selected_candidate_index: usize) {
        self.viewport.next_expanded_item(selected_candidate_index);
    }

    /// 在二级展开卡片中向上移动选中项
    pub fn prev_expanded_item(&mut self, selected_candidate_index: usize) {
        self.viewport.prev_expanded_item(selected_candidate_index);
    }

    /// 在二级展开卡片中选中子项上屏
    pub fn select_expanded_item(
        &mut self,
        main_index: usize,
        sub_index: usize,
    ) -> Option<(String, usize)> {
        let cand = self.viewport.current_candidates.get(main_index)?;
        let raw_text = cand.sub_candidates.get(sub_index)?.clone();

        // 盘古之白（中西文智能空格）：自动在交界处补充半角空格
        let commit_text = if self.config.auto_pangu_spacing {
            if let Some(prev) = self.composition.last_commit_context() {
                if PanguSpacing::should_insert_pangu_space(prev, &raw_text) {
                    format!(" {}", raw_text)
                } else {
                    raw_text.clone()
                }
            } else {
                raw_text.clone()
            }
        } else {
            raw_text.clone()
        };

        let trimmed_text = raw_text.trim();
        if !trimmed_text.is_empty() {
            self.composition
                .set_last_commit_context(Some(trimmed_text.to_string()));
        }

        self.composition.reset();
        self.viewport.reset();

        Some((commit_text, 0))
    }

    /// 精确计算用户选词所消费的原始按键长度（含自动跳过隔音符）
    pub(crate) fn compute_consumed_len(raw: &str, pinyin: &str) -> usize {
        if raw.starts_with(pinyin) {
            let mut len = pinyin.len();
            if raw.is_char_boundary(len) && raw[len..].starts_with('\'') {
                len = len.saturating_add(1);
            }
            return len;
        }

        let clean_pinyin: String = pinyin.chars().filter(|&c| c != '\'').collect();
        let mut pinyin_iter = clean_pinyin.chars();
        let mut target_opt = pinyin_iter.next();
        let mut last_end = 0;

        for (idx, ch) in raw.char_indices() {
            if let Some(target) = target_opt {
                if ch.eq_ignore_ascii_case(&target) {
                    last_end = idx.saturating_add(ch.len_utf8());
                    target_opt = pinyin_iter.next();
                } else if ch == '\'' {
                    last_end = idx.saturating_add(ch.len_utf8());
                } else {
                    break;
                }
            } else {
                if ch == '\'' {
                    last_end = idx.saturating_add(ch.len_utf8());
                }
                break;
            }
        }

        if target_opt.is_none() {
            last_end
        } else {
            0
        }
    }
}
