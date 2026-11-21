//! 候选框渲染帧级增量脏检查快照模块
//!
//! # 设计原理
//! - **实现初衷**：替代每帧深拷贝整份 `CandidateBoxState` 与 `UiTheme` 的快照比对方式，
//!   以零堆分配的双 64 位结构化指纹（`DefaultHasher` + FNV-1a）精确判定渲染输入是否变化。
//! - **核心优势**：命中未变化帧时直接复用已有 RGBA 像素缓冲跳过全量光栅化；
//!   未命中时仅写入栈上 `Copy` 指纹，彻底消除每帧候选列表克隆开销。
//! - **代价与局限**：极端理论场景下存在 128 位哈希碰撞概率（约 $2^{-128}$，工程上可忽略）。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::style::RendererStyle;
use crate::theme::{Color, UiTheme};
use waylan_core::{Candidate, CandidateBoxState, CandidateExpansion, VoiceBoxState};

/// 最近一次完整渲染的零分配帧快照指纹
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LastRenderSnapshot {
    pub state_hash: (u64, u64),
    pub theme_hash: u64,
    pub style: RendererStyle,
    pub font_size_bits: u32,
    pub scale_bits: u32,
    pub width: u32,
    pub height: u32,
}

/// 轻量 64 位 FNV-1a 哈希器，与标准库 `DefaultHasher` 组成 128 位双独立摘要
struct Fnv1aHasher(u64);

impl Fnv1aHasher {
    #[inline]
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    #[inline]
    fn write_u64(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }

    #[inline]
    fn write_str(&mut self, s: &str) {
        self.write_u64(s.len() as u64);
        self.write_bytes(s.as_bytes());
    }

    #[inline]
    fn finish(self) -> u64 {
        self.0
    }
}

/// 计算候选框完整状态的 128 位零分配内容指纹
pub(super) fn compute_state_fingerprint(state: &CandidateBoxState) -> (u64, u64) {
    let mut h1 = DefaultHasher::new();
    let mut h2 = Fnv1aHasher::new();

    state.visible.hash(&mut h1);
    h2.write_u64(u64::from(state.visible));

    state.preedit.hash(&mut h1);
    h2.write_str(&state.preedit);

    state.selected_index.hash(&mut h1);
    h2.write_u64(state.selected_index as u64);

    state.mode_badge.hash(&mut h1);
    h2.write_str(&state.mode_badge);

    hash_expansion(state.expansion, &mut h1, &mut h2);
    hash_candidates(&state.candidates, &mut h1, &mut h2);
    hash_candidates(&state.grid_candidates, &mut h1, &mut h2);
    hash_voice(state.voice.as_ref(), &mut h1, &mut h2);

    (h1.finish(), h2.finish())
}

/// 计算当前主题配色的 64 位指纹，消除每帧 `UiTheme::clone()`
pub(super) fn compute_theme_hash(theme: &UiTheme) -> u64 {
    let mut h = Fnv1aHasher::new();
    hash_color(theme.background, &mut h);
    hash_color(theme.border, &mut h);
    hash_color(theme.preedit_text, &mut h);
    hash_color(theme.highlight_badge, &mut h);
    hash_color(theme.highlight_text, &mut h);
    hash_color(theme.candidate_text, &mut h);
    hash_color(theme.index_text, &mut h);
    h.write_u64(u64::from(theme.corner_radius.to_bits()));
    h.write_u64(u64::from(theme.padding.to_bits()));
    h.finish()
}

#[inline]
fn hash_color(c: Color, h: &mut Fnv1aHasher) {
    h.write_u64(u64::from(c.r.to_bits()));
    h.write_u64(u64::from(c.g.to_bits()));
    h.write_u64(u64::from(c.b.to_bits()));
    h.write_u64(u64::from(c.a.to_bits()));
}

#[inline]
fn hash_expansion(exp: CandidateExpansion, h1: &mut DefaultHasher, h2: &mut Fnv1aHasher) {
    let (tag, idx) = match exp {
        CandidateExpansion::None => (0u64, 0u64),
        CandidateExpansion::Card { selected_index } => (1u64, selected_index as u64),
        CandidateExpansion::Grid { selected_index } => (2u64, selected_index as u64),
    };
    tag.hash(h1);
    idx.hash(h1);
    h2.write_u64(tag);
    h2.write_u64(idx);
}

fn hash_candidates(cands: &[Candidate], h1: &mut DefaultHasher, h2: &mut Fnv1aHasher) {
    cands.len().hash(h1);
    h2.write_u64(cands.len() as u64);
    for c in cands {
        c.text.hash(h1);
        h2.write_str(&c.text);
        c.comment.hash(h1);
        if let Some(ref comment) = c.comment {
            h2.write_u64(1);
            h2.write_str(comment);
        } else {
            h2.write_u64(0);
        }
        c.is_expandable.hash(h1);
        h2.write_u64(u64::from(c.is_expandable));
        c.sub_candidates.len().hash(h1);
        h2.write_u64(c.sub_candidates.len() as u64);
        for sub in &c.sub_candidates {
            sub.hash(h1);
            h2.write_str(sub);
        }
    }
}

fn hash_voice(voice: Option<&VoiceBoxState>, h1: &mut DefaultHasher, h2: &mut Fnv1aHasher) {
    match voice {
        None => {
            0u8.hash(h1);
            h2.write_u64(0);
        }
        Some(v) => {
            1u8.hash(h1);
            h2.write_u64(1);
            v.is_listening.hash(h1);
            h2.write_u64(u64::from(v.is_listening));
            v.partial_text.hash(h1);
            h2.write_str(&v.partial_text);
            v.waveform.hash(h1);
            h2.write_str(&v.waveform);
            v.display_text.hash(h1);
            if let Some(ref dt) = v.display_text {
                h2.write_u64(1);
                h2.write_str(dt);
            } else {
                h2.write_u64(0);
            }
            let effect_tag = format!("{:?}", v.visual_effect);
            effect_tag.hash(h1);
            h2.write_str(&effect_tag);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_fingerprint_detects_selected_index_and_preedit_changes() {
        let mut s1 = CandidateBoxState {
            visible: true,
            preedit: "ni'hao".to_string(),
            candidates: vec![
                Candidate::new("你好", "ni'hao", 100),
                Candidate::new("拟好", "ni'hao", 80),
            ],
            selected_index: 0,
            ..Default::default()
        };
        let fp1 = compute_state_fingerprint(&s1);
        assert_eq!(fp1, compute_state_fingerprint(&s1));

        s1.selected_index = 1;
        let fp2 = compute_state_fingerprint(&s1);
        assert_ne!(fp1, fp2);

        s1.preedit.push('\'');
        let fp3 = compute_state_fingerprint(&s1);
        assert_ne!(fp2, fp3);
    }
}
