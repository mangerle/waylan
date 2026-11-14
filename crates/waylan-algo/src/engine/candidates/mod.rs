//! 候选词重新计算、模式分流与统一束搜索流水线装配
//!
//! # 设计原理
//! - **实现初衷**：负责增量拼写击键发生时，协调前缀模式分流、统一音节格构建、
//!   按字节位置同步束搜索解码与后处理流水线。
//! - **核心优势**：以单次 `SyllableLattice` + `PositionalBeamDecoder` 取代多层路径枚举，
//!   复杂度与单文件行数严格受控。
//! - **代价与局限**：受束宽与单次词组最大音节数（6 音节）限制。

mod collector;

use super::AlgoEngine;
use crate::decoder::{
    BeamDecodeOptions, DecodeResult, LatticeOptions, MatcherOptions, PositionalBeamDecoder,
    SyllableLattice,
};
use crate::dispatcher::ModeDispatcher;
use crate::pipeline::CandidatePipeline;
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

        // 2. 拼音模式：构建统一音节格并执行端到端束搜索解码
        let (lattice, decode) = self.build_lattice_and_decode();
        let preedit_str = if decode.best_preedit.is_empty() {
            self.composition.as_str().to_string()
        } else {
            decode.best_preedit.clone()
        };
        self.composition.set_cached_preedit(preedit_str);

        // 3. 收集多层级候选并叠加用户学习权重
        let raw_candidates = self.collect_all_raw_candidates(&lattice, &decode);

        // 4. 流水线后处理、零页扩展注入与候选发布
        self.finalize_and_publish_candidates(&decode, raw_candidates);
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

    /// 构建统一音节格并执行按字节位置同步束搜索解码
    pub(crate) fn build_lattice_and_decode(&self) -> (SyllableLattice, DecodeResult) {
        let raw = self.composition.as_str();
        let lattice_opts = LatticeOptions {
            fuzzy: self.config.fuzzy,
            typo_rules: self.config.typo_rules,
            enable_mixed_jianpin: self.config.enable_acronym,
            enable_typo_edges: self.config.typo_rules.enable,
        };
        let lattice = SyllableLattice::build(raw, &lattice_opts);

        let beam_opts = BeamDecodeOptions {
            beam_width: 24,
            max_typos: 2,
            max_sentences: 8,
            matcher_options: MatcherOptions {
                max_syllables: 6,
                max_typos: 2,
                top_k_per_key: 12,
                max_initial_steps: 4,
                filter_rare_ext_chars: self.config.filter_rare_ext_chars,
            },
            weights: self.config.scoring_weights,
            eval_ts: self.adaptive.last_timestamp,
            last_commit_context: self.composition.last_commit_context(),
        };
        let decode = PositionalBeamDecoder::decode(
            &lattice,
            &self.resources.table,
            Some(&self.adaptive.model),
            &beam_opts,
        );
        (lattice, decode)
    }

    /// 候选词流水线最终处理（去重、简繁、零页扩展、标记 consumed_len）并生效
    fn finalize_and_publish_candidates(
        &mut self,
        decode: &DecodeResult,
        raw_candidates: Vec<AlgoCandidate>,
    ) {
        let raw_input = self.composition.as_str();
        let best_units: Vec<String> = decode
            .best_preedit
            .split('\'')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        let mut candidates = CandidatePipeline::process_all_with_opencc(
            raw_candidates,
            &self.config,
            self.config
                .traditional_chinese
                .then(|| self.resources.opencc_dict()),
        );

        self.inject_page_zero_extensions(raw_input, &best_units, &mut candidates);

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
