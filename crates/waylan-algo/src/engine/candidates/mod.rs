//! 候选词重新计算、模式分流与流水线装配
//!
//! # 设计原理
//! - **实现初衷**：负责增量拼写击键发生时，协调前缀模式分流、DAG 路径构建、多源候选聚合与后处理流水线。
//! - **核心优势**：高内聚状态机协调，将具体的多级多源候选收集算法拆解至 `collector` 子模块，复杂度与单文件行数严格受控。
//! - **代价与局限**：在复杂输入时需跨多层模块调度。

mod collector;

use super::AlgoEngine;
use crate::dispatcher::ModeDispatcher;
use crate::pipeline::CandidatePipeline;
use crate::prism::{PrismEngine, SegmentPath};
use crate::types::AlgoCandidate;

impl AlgoEngine {
    /// 重新计算当前输入序列的全量候选词列表并分页刷新
    pub(crate) fn recompute_candidates(&mut self) {
        if self.composition.is_empty() {
            self.clear_candidates_and_preedit();
            return;
        }

        // 1. 尝试前缀模式分流 (V计算器、U拆字、I特殊符号)
        if self.try_dispatch_special_mode() {
            return;
        }

        // 2. 拼音模式：利用 Prism 构建 DAG 音节路径与预编辑字符串
        let (paths, preedit_str) = self.build_pinyin_paths_and_preedit();
        self.composition.set_cached_preedit(preedit_str);

        // 3. 聚合各层级候选词 (专名、DAG路径、长句合成、简拼与用户习惯)
        let raw_candidates = self.collect_all_raw_candidates(&paths);

        // 4. 流水线后处理、零页扩展注入与候选发布
        self.finalize_and_publish_candidates(&paths, raw_candidates);
    }

    /// 清空所有候选词缓存与预编辑内容
    fn clear_candidates_and_preedit(&mut self) {
        self.candidates.clear();
        self.composition.clear_cached_preedit();
    }

    /// 尝试前缀特殊模式分流 (V计算器、U拆字、I特殊符号)，若命中则更新候选并返回 true
    fn try_dispatch_special_mode(&mut self) -> bool {
        let Some(dispatched) =
            ModeDispatcher::try_dispatch(self.composition.as_str(), &self.config)
        else {
            return false;
        };

        let input_len = self.composition.len();
        let raw_str = self.composition.as_str().to_string();
        self.composition.set_cached_preedit(raw_str);
        let mut cands = CandidatePipeline::process_all_with_opencc(
            dispatched,
            &self.config,
            self.config
                .traditional_chinese
                .then(|| self.resources.opencc_dict()),
        );
        for c in &mut cands {
            if c.consumed_len.is_none() {
                c.consumed_len = Some(input_len);
            }
        }
        self.candidates = cands;
        true
    }

    /// 基于 Prism 构建音节切分 DAG 路径与预编辑字符串
    fn build_pinyin_paths_and_preedit(&self) -> (Vec<SegmentPath>, String) {
        let raw = self.composition.as_str();
        let paths = PrismEngine::build_paths_with_options(
            raw,
            &self.config.fuzzy,
            &self.config.typo_rules,
            Some(&self.resources.entity_manager),
        );
        let preedit_str = paths
            .first()
            .map(|p| self.format_head_path_preedit(p, &paths, raw))
            .unwrap_or_else(|| raw.to_string());
        (paths, preedit_str)
    }

    /// 格式化首条切分路径为预编辑字符串
    ///
    /// # 设计原理
    /// - **实现初衷**：当首条路径为"碎切容错回退"结果时，其音节序列不可直接展示，
    ///   必须改取第一条完整闭合的容错路径原始拼写，否则预编辑区会显示错误断句。
    /// - **核心优势**：正常路径与碎切回退路径分别取用最适合的呈现形式，避免用户看到误导性分段。
    /// - **代价与局限**：完整容错路径缺失时仍退回首条路径的键串呈现。
    fn format_head_path_preedit(
        &self,
        head_path: &SegmentPath,
        paths: &[SegmentPath],
        raw: &str,
    ) -> String {
        if !head_path.is_broken_typo_fallback(&self.config.fuzzy) {
            return head_path.to_key_string();
        }
        paths
            .iter()
            .find(|tp| tp.is_typo() && tp.remaining.is_empty())
            .and_then(|tp| tp.format_raw_with_spans(raw))
            .unwrap_or_else(|| head_path.to_key_string())
    }

    /// 候选词流水线最终处理（去重、简繁、零页扩展、标记 consumed_len）并分页生效
    fn finalize_and_publish_candidates(
        &mut self,
        paths: &[SegmentPath],
        raw_candidates: Vec<AlgoCandidate>,
    ) {
        let raw_input = self.composition.as_str();
        let mut best_units: Vec<String> = Vec::new();
        if let Some(best) = paths.first() {
            best_units.extend(best.syllables.iter().cloned());
            if !best.remaining.is_empty() {
                best_units.push(best.remaining.clone());
            }
        }

        let mut candidates = CandidatePipeline::process_all_with_opencc(
            raw_candidates,
            &self.config,
            self.config
                .traditional_chinese
                .then(|| self.resources.opencc_dict()),
        );

        self.inject_page_zero_extensions(raw_input, paths, &mut candidates);

        for cand in &mut candidates {
            if cand.consumed_len.is_none() {
                cand.consumed_len = Some(Self::calculate_consumed_len(
                    raw_input,
                    &cand.pinyin,
                    &best_units,
                ));
            }
        }

        let old_cache = std::mem::replace(&mut self.candidates, candidates);
        self.scratch_candidates = old_cache;
    }
}
