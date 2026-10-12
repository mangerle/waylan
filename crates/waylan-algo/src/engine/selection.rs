use super::pangu::PanguSpacing;
use super::AlgoEngine;
use crate::dispatcher::ModeDispatcher;
use crate::prism::PrismEngine;
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
        if selected.is_expandable
            || selected.pinyin.is_empty()
            || selected.text.is_empty()
            || self.config.traditional_chinese
        {
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
        if index >= self.current_candidates.len() {
            return None;
        }
        let selected = self.current_candidates[index].clone();
        Some(self.commit_candidate_internal(selected, current_timestamp))
    }

    /// 从当前输入的全量候选词缓存中直接选取指定全局索引的词条上屏
    pub fn select_candidate_from_all(
        &mut self,
        global_index: usize,
        current_timestamp: u64,
    ) -> Option<(String, usize)> {
        if global_index >= self.all_candidates_cache.len() {
            return None;
        }
        let selected = self.all_candidates_cache[global_index].clone();
        Some(self.commit_candidate_internal(selected, current_timestamp))
    }

    /// 核心提交逻辑内部实现：自造词、调频、盘古之白与消费前缀
    fn commit_candidate_internal(
        &mut self,
        selected: AlgoCandidate,
        current_timestamp: u64,
    ) -> (String, usize) {
        self.last_timestamp = self.last_timestamp.max(current_timestamp);
        let should_record = self.should_record_to_user_dict(&selected);

        // 若选中的是可展开项，且直接按数字键确认（未展开时）：
        // 优先将首个格式化子候选文本（如当前标准日期 2026年10月9日）上屏
        let raw_commit = if selected.is_expandable && !selected.sub_candidates.is_empty() {
            selected.sub_candidates[0].clone()
        } else {
            selected.text.clone()
        };

        // 仅当选中的候选词为标准简体中文拼音词条时，才记录用户选词并动态调频
        if should_record {
            if let Some(ref path) = self.user_dict_path {
                let _ = self.user_model.append_to_wal(
                    path,
                    &selected.text,
                    &selected.pinyin,
                    current_timestamp,
                );
            } else {
                self.user_model
                    .record_commit(&selected.text, &selected.pinyin, current_timestamp);
            }
        }

        // 盘古之白（中西文智能空格）：自动在交界处补充半角空格
        let commit_text = if self.config.auto_pangu_spacing {
            if let Some(prev) = &self.last_commit_context {
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
            self.last_commit_context = Some(trimmed_commit.to_string());
        }

        // 计算消费的拼音字母长度并执行分段确认（动态功能项、容错纠错项与特殊模式分发项全量消费输入）
        let consumed_len = if let Some(len) = selected.consumed_len {
            len.min(self.composition.len())
        } else if selected.is_expandable
            || selected.tier == CandidateTier::TypoCorrection
            || self.composition.as_str().starts_with('v')
            || ModeDispatcher::try_dispatch(self.composition.as_str(), &self.config).is_some()
        {
            self.composition.len()
        } else {
            self.compute_consumed_len_for_candidate(&selected)
        };

        if should_record {
            self.partial_commit_segments
                .push((selected.text.clone(), selected.pinyin.clone()));
        } else {
            self.partial_commit_segments.clear();
        }

        self.composition.consume_prefix(consumed_len);

        // 若当前整串拼音已全部消费完毕，且通过 2 次及以上分段选词完成，自动合成为用户自造新词
        if self.composition.is_empty() {
            if self.partial_commit_segments.len() >= 2 {
                let full_text: String = self
                    .partial_commit_segments
                    .iter()
                    .map(|(t, _)| t.as_str())
                    .collect();
                let full_pinyin: String = self
                    .partial_commit_segments
                    .iter()
                    .map(|(_, p)| p.as_str())
                    .collect::<Vec<_>>()
                    .join("'");
                let char_len = full_text.chars().count();
                if (2..=8).contains(&char_len) {
                    if let Some(ref path) = self.user_dict_path {
                        let _ = self.user_model.append_to_wal(
                            path,
                            &full_text,
                            &full_pinyin,
                            current_timestamp,
                        );
                    } else {
                        self.user_model
                            .record_commit(&full_text, &full_pinyin, current_timestamp);
                    }
                }
            }
            self.partial_commit_segments.clear();
        }

        self.page_index = 0;
        if self.composition.is_empty() {
            self.all_candidates_cache.clear();
            self.current_candidates.clear();
            self.cached_preedit.clear();
            self.selected_index = 0;
            self.is_expanded = false;
            self.expanded_selected_index = 0;
        } else {
            self.recompute_candidates();
        }

        (commit_text, self.composition.len())
    }

    /// 切换二级扩展卡片的展开状态（仅当当前选中的项支持二级展开时有效）
    pub fn toggle_expand(&mut self, selected_candidate_index: usize) -> bool {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if cand.is_expandable && !cand.sub_candidates.is_empty() {
                self.is_expanded = !self.is_expanded;
                self.expanded_selected_index = 0;
                return self.is_expanded;
            }
        }
        self.is_expanded = false;
        false
    }

    /// 在二级展开卡片中向下移动选中项
    pub fn next_expanded_item(&mut self, selected_candidate_index: usize) {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if !cand.sub_candidates.is_empty() {
                self.expanded_selected_index =
                    (self.expanded_selected_index + 1) % cand.sub_candidates.len();
            }
        }
    }

    /// 在二级展开卡片中向上移动选中项
    pub fn prev_expanded_item(&mut self, selected_candidate_index: usize) {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if !cand.sub_candidates.is_empty() {
                if self.expanded_selected_index == 0 {
                    self.expanded_selected_index = cand.sub_candidates.len().saturating_sub(1);
                } else {
                    self.expanded_selected_index = self.expanded_selected_index.saturating_sub(1);
                }
            }
        }
    }

    /// 在二级展开卡片中选中子项上屏
    pub fn select_expanded_item(
        &mut self,
        main_index: usize,
        sub_index: usize,
    ) -> Option<(String, usize)> {
        let cand = self.current_candidates.get(main_index)?;
        let raw_text = cand.sub_candidates.get(sub_index)?.clone();

        // 盘古之白（中西文智能空格）：自动在交界处补充半角空格
        let commit_text = if self.config.auto_pangu_spacing {
            if let Some(prev) = &self.last_commit_context {
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
            self.last_commit_context = Some(trimmed_text.to_string());
        }

        self.partial_commit_segments.clear();
        self.composition.clear();
        self.page_index = 0;
        self.all_candidates_cache.clear();
        self.current_candidates.clear();
        self.cached_preedit.clear();
        self.selected_index = 0;
        self.is_expanded = false;
        self.expanded_selected_index = 0;

        Some((commit_text, 0))
    }

    /// 结合当前双拼/全拼模式与音节图计算选中候选词所消费的原始按键字节长度
    fn compute_consumed_len_for_candidate(&self, selected: &AlgoCandidate) -> usize {
        let raw = self.composition.as_str();
        let pinyin = selected.pinyin.as_str();
        if raw.is_empty() {
            return 0;
        }

        if raw.starts_with(pinyin) && !pinyin.is_empty() {
            let mut len = pinyin.len();
            if raw[len..].starts_with('\'') {
                len = len.saturating_add(1);
            }
            return len;
        }

        // 全拼与简拼混合模式：优先按候选音节逐段对齐原始输入或最优切分路径单元
        let cand_syls: Vec<&str> = pinyin.split('\'').filter(|s| !s.is_empty()).collect();
        let paths = PrismEngine::build_paths_with_entities(
            raw,
            &self.config.fuzzy,
            Some(&self.entity_manager),
        );
        let mut best_units: Vec<String> = Vec::new();
        if let Some(best) = paths.first() {
            best_units.extend(best.syllables.iter().cloned());
            if !best.remaining.is_empty() {
                best_units.push(best.remaining.clone());
            }
        }

        let mut pos = 0usize;
        for (i, &cs) in cand_syls.iter().enumerate() {
            while pos < raw.len() && raw[pos..].starts_with('\'') {
                pos += 1;
            }
            if pos >= raw.len() {
                break;
            }
            if raw[pos..].starts_with(cs) {
                pos += cs.len();
            } else if i < best_units.len() && raw[pos..].starts_with(best_units[i].as_str()) {
                pos += best_units[i].len();
            } else {
                let first_len = cs.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                if raw[pos..].starts_with(&cs[..first_len]) {
                    pos += first_len;
                } else {
                    return Self::compute_consumed_len(raw, pinyin);
                }
            }
            while pos < raw.len() && raw[pos..].starts_with('\'') {
                pos += 1;
            }
        }

        if pos > 0 {
            pos.min(raw.len())
        } else {
            Self::compute_consumed_len(raw, pinyin)
        }
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
