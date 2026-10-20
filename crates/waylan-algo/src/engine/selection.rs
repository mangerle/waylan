use super::AlgoEngine;
use crate::dispatcher::ModeDispatcher;
use crate::types::{AlgoCandidate, CandidateTier};
use waylan_core::{is_cjk_character, PanguSpacing};

/// 分段选词记录收尾处理的参数对象
///
/// 承载分段选词收尾所需的候选文本、候选拼音、是否记录标志与提交时间戳。
struct PartialSegmentFinalizeParams {
    /// 本次选中的候选文本
    selected_text: String,
    /// 本次选中的候选拼音
    selected_pinyin: String,
    /// 是否需要记录到用户自适应词频模型
    should_record: bool,
    /// 本次提交动作的时间戳
    current_timestamp: u64,
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
        selected.text.chars().all(is_cjk_character)
    }

    pub fn select_candidate(
        &mut self,
        index: usize,
        current_timestamp: u64,
    ) -> Option<(String, usize)> {
        if index >= self.candidates.len() {
            return None;
        }
        let selected = self.candidates[index].clone();
        Some(self.commit_candidate_internal(selected, current_timestamp))
    }

    /// 从当前输入的全量候选词缓存中直接选取指定全局索引的词条上屏（兼容接口）
    pub fn select_candidate_from_all(
        &mut self,
        global_index: usize,
        current_timestamp: u64,
    ) -> Option<(String, usize)> {
        self.select_candidate(global_index, current_timestamp)
    }

    /// 准备上屏文本并根据盘古之白规则注入空格，同时更新跨句历史上下文
    fn prepare_commit_text(&mut self, selected: &mut AlgoCandidate, should_record: bool) -> String {
        let raw_commit = if selected.is_expandable && !selected.sub_candidates.is_empty() {
            selected.sub_candidates.swap_remove(0)
        } else if should_record {
            selected.text.clone()
        } else {
            std::mem::take(&mut selected.text)
        };

        let trimmed_commit = raw_commit.trim();
        let last_context = if !trimmed_commit.is_empty() {
            Some(trimmed_commit.to_string())
        } else {
            None
        };

        let commit_text = if self.config.auto_pangu_spacing {
            if let Some(prev) = self.composition.last_commit_context() {
                if PanguSpacing::should_insert_pangu_space(prev, &raw_commit) {
                    format!(" {}", raw_commit)
                } else {
                    raw_commit
                }
            } else {
                raw_commit
            }
        } else {
            raw_commit
        };

        if let Some(ctx) = last_context {
            self.composition.set_last_commit_context(Some(ctx));
        }

        commit_text
    }

    /// 计算选词提交需要消费的拼音长度，具备多级对齐与防死锁保底策略
    fn calculate_candidate_consumed_len(&self, selected: &AlgoCandidate) -> usize {
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

        if consumed_len == 0 {
            log::warn!(
                "[算法引擎] 选词提交消费长度为 0，触发安全保底消费全部拼音: 词条=\"{}\", 拼音=\"{}\", 剩余拼音=\"{}\"",
                selected.text,
                selected.pinyin,
                self.composition.as_str()
            );
            consumed_len = self.composition.len();
        }

        consumed_len
    }

    /// 处理分段选词记录与多音节用户自造词自动合成
    fn finalize_partial_segments(&mut self, params: PartialSegmentFinalizeParams) {
        let PartialSegmentFinalizeParams {
            selected_text,
            selected_pinyin,
            should_record,
            current_timestamp,
        } = params;

        if should_record {
            self.composition
                .record_partial_segment(selected_text, selected_pinyin);
        } else {
            self.composition.clear_partial_segments();
        }

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
    }

    /// 核心提交逻辑内部实现：自造词、调频、盘古之白与消费前缀
    fn commit_candidate_internal(
        &mut self,
        mut selected: AlgoCandidate,
        current_timestamp: u64,
    ) -> (String, usize) {
        self.adaptive.last_timestamp = self.adaptive.last_timestamp.max(current_timestamp);
        let should_record = self.should_record_to_user_dict(&selected);

        if should_record {
            self.adaptive
                .record_commit(&selected.text, &selected.pinyin, current_timestamp);
        }

        let commit_text = self.prepare_commit_text(&mut selected, should_record);
        let consumed_len = self.calculate_candidate_consumed_len(&selected);

        self.composition.consume_prefix(consumed_len);
        self.finalize_partial_segments(PartialSegmentFinalizeParams {
            selected_text: selected.text,
            selected_pinyin: selected.pinyin,
            should_record,
            current_timestamp,
        });

        if self.composition.is_empty() {
            self.candidates.clear();
            self.composition.clear_cached_preedit();
        } else {
            self.recompute_candidates();
        }

        (commit_text, self.composition.len())
    }

    /// 直接提交指定文本上屏并消费全部拼音（用于二级扩展项、自定义短语等直接上屏）
    pub fn commit_custom_text(&mut self, raw_text: &str) -> (String, usize) {
        let commit_text = if self.config.auto_pangu_spacing {
            if let Some(prev) = self.composition.last_commit_context() {
                if PanguSpacing::should_insert_pangu_space(prev, raw_text) {
                    format!(" {}", raw_text)
                } else {
                    raw_text.to_string()
                }
            } else {
                raw_text.to_string()
            }
        } else {
            raw_text.to_string()
        };

        let trimmed_text = raw_text.trim();
        if !trimmed_text.is_empty() {
            self.composition
                .set_last_commit_context(Some(trimmed_text.to_string()));
        }

        self.composition.reset();
        self.candidates.clear();

        (commit_text, 0)
    }

    /// 在二级展开卡片中选中子项上屏（兼容辅助方法）
    pub fn select_expanded_item(
        &mut self,
        main_index: usize,
        sub_index: usize,
    ) -> Option<(String, usize)> {
        let cand = self.candidates.get(main_index)?;
        let raw_text = cand.sub_candidates.get(sub_index)?.clone();
        Some(self.commit_custom_text(&raw_text))
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

    /// 结合当前最优切分路径单元计算候选词所消费的原始按键字节长度
    ///
    /// # 设计原理
    /// - **实现初衷**：在候选词生成阶段，利用已经求解出的音节路径 `best_units` 预先精确计算
    ///   每个候选词所消费的按键长度，并在候选词中记录 `consumed_len`。
    /// - **核心优势**：彻底消除用户选词上屏时重新调用 `PrismEngine::build_paths_with_entities`
    ///   求解音节图路径的微秒级延迟，将按键上屏关键交互路径耗时降为纯内存字段读取。
    pub(crate) fn calculate_consumed_len(raw: &str, pinyin: &str, best_units: &[String]) -> usize {
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
}
