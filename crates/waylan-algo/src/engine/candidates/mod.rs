//! 候选词重新计算、模式分流与统一束搜索流水线装配
//!
//! # 设计原理
//! - **实现初衷**：负责增量拼写击键发生时，协调前缀模式分流、统一音节格构建、
//!   按字节位置同步束搜索解码与后处理流水线。
//! - **核心优势**：以单次 `SyllableLattice` + `PositionalBeamDecoder` 取代多层路径枚举，
//!   复杂度与单文件行数严格受控。
//! - **代价与局限**：受束宽与单次词组最大音节数（6 音节）限制。

mod collector;
mod fallback;

use super::AlgoEngine;
use crate::decoder::{
    BeamDecodeOptions, DecodeResult, LatticeOptions, MatcherOptions, PositionalBeamDecoder,
    SyllableLattice, compute_decode_config_sig,
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

        // 3. 收集多层级候选并叠加用户学习权重（复用 scratch_candidates 容量）
        let mut raw_candidates = std::mem::take(&mut self.scratch_candidates);
        self.collect_all_raw_candidates_into(&lattice, &decode, &mut raw_candidates);

        // 4. 流水线后处理、零页扩展注入与候选发布
        self.finalize_and_publish_candidates(&decode, raw_candidates);
    }

    /// 清空所有候选词缓存与预编辑内容
    fn clear_candidates_and_preedit(&mut self) {
        self.candidates.clear();
        self.prepared_up_to = 0;
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
    pub(crate) fn build_lattice_and_decode(&mut self) -> (SyllableLattice, DecodeResult) {
        let raw = self.composition.as_str();
        let lattice_opts = LatticeOptions {
            fuzzy: self.config.fuzzy,
            typo_rules: self.config.typo_rules,
            enable_mixed_jianpin: self.config.enable_acronym,
            enable_typo_edges: self.config.typo_rules.enable,
        };

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
        let f = &self.config.fuzzy;
        let fuzzy_mask = (u16::from(f.c_ch))
            | (u16::from(f.z_zh) << 1)
            | (u16::from(f.s_sh) << 2)
            | (u16::from(f.an_ang) << 3)
            | (u16::from(f.en_eng) << 4)
            | (u16::from(f.in_ing) << 5)
            | (u16::from(f.l_n) << 6)
            | (u16::from(f.f_h) << 7)
            | (u16::from(f.r_l) << 8)
            | (u16::from(f.g_k) << 9);
        let config_sig = compute_decode_config_sig(
            fuzzy_mask,
            self.config.typo_rules.enable,
            self.config.enable_acronym,
            &beam_opts,
        );
        let lattice = SyllableLattice::build_incremental(
            raw,
            &lattice_opts,
            self.beam_cache.cached_lattice(config_sig),
        );
        let decode = PositionalBeamDecoder::decode_with_cache(
            &lattice,
            &self.resources.table,
            Some(&self.adaptive.model),
            &beam_opts,
            &mut self.beam_cache,
            config_sig,
        );
        self.beam_cache.store_lattice(lattice.clone());
        (lattice, decode)
    }

    /// 候选词流水线最终处理（去重、简繁、零页扩展、标记 consumed_len）并生效
    fn finalize_and_publish_candidates(
        &mut self,
        decode: &DecodeResult,
        raw_candidates: Vec<AlgoCandidate>,
    ) {
        let raw_input = self.composition.as_str();
        let best_units: Vec<&str> = decode
            .best_preedit
            .split('\'')
            .filter(|s| !s.is_empty())
            .collect();

        let mut candidates = CandidatePipeline::process_core_with_context(
            raw_candidates,
            &self.config,
            self.config
                .traditional_chinese
                .then(|| self.resources.opencc_dict()),
            Some(raw_input),
        );

        self.inject_page_zero_extensions(raw_input, &best_units, &mut candidates);

        // 仅对首页可见窗口预先执行拼音注释挂载与 consumed_len 计算，其余页延迟至翻页/展开时按需处理
        let initial_window = self.config.page_size.max(7).min(candidates.len());
        if self.config.show_pinyin_comment {
            CandidatePipeline::decorate_comments(&mut candidates[..initial_window]);
        }
        for cand in &mut candidates[..initial_window] {
            if cand.consumed_len.is_none() {
                cand.consumed_len = Some(Self::calculate_consumed_len(
                    raw_input,
                    &cand.pinyin,
                    &best_units,
                ));
            }
        }
        let mut recycled = std::mem::replace(&mut self.candidates, candidates);
        recycled.clear();
        self.scratch_candidates = recycled;
        self.prepared_up_to = initial_window;
    }

    /// 按需补齐 `[0..end]` 范围内候选词的拼音注释与消费长度标记（供翻页与大网格展开调用）
    pub fn prepare_candidates_up_to(&mut self, end: usize) {
        let target = end.min(self.candidates.len());
        if target <= self.prepared_up_to {
            return;
        }
        let start = self.prepared_up_to;
        if self.config.show_pinyin_comment {
            CandidatePipeline::decorate_comments(&mut self.candidates[start..target]);
        }
        let raw_input = self.composition.as_str();
        let best_units: Vec<&str> = self
            .composition
            .cached_preedit()
            .split('\'')
            .filter(|s| !s.is_empty())
            .collect();
        for cand in &mut self.candidates[start..target] {
            if cand.consumed_len.is_none() {
                cand.consumed_len = Some(Self::calculate_consumed_len(
                    raw_input,
                    &cand.pinyin,
                    &best_units,
                ));
            }
        }
        self.prepared_up_to = target;
    }
}
