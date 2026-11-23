//! 统一音节格的四类容错边生成器
//!
//! # 设计原理
//! - **实现初衷**：将单音节邻键/漏按/多按、单音节内相邻字符倒置、跨音节字符倒置以及
//!   边界共享尾字母漏按统一抽象为独立生成模块，保持 `SyllableLattice` 主文件清晰精简。
//! - **核心优势**：所有容错边与标准音节边共享相同的 `SyllableEdge` 结构，在束搜索中直接参与竞争。
//! - **代价与局限**：跨音节倒置窗口限制在 12 字节以内，以控制长串构造开销。

use super::fsa::PrismFsa;
use super::lattice::{EdgeKind, EdgeSyllables, LatticeOptions, SyllableEdge, TypoKind};
use crate::syllables::is_valid_syllable;
use crate::syllables::spatial::{
    get_spatial_typo_corrections, get_typo_corrections_with_rules, weighted_edit_distance,
};

/// 生成四类容错边（含单音节空间纠错、跨音节倒置、边界共享漏按）
pub(super) fn populate_typo_edges(
    start: usize,
    seg: &str,
    options: &LatticeOptions,
    out: &mut Vec<SyllableEdge>,
) {
    // 1. 单音节内邻键/倒置/漏按/多按（纯辅音串不参与单音节邻键猜测，避免将简拼如 gzn 误判为 gen）
    let max_len = seg.len().min(7);
    for l in 3..=max_len {
        let slice = &seg[..l];
        if is_valid_syllable(slice)
            || !slice
                .bytes()
                .any(|b| matches!(b, b'a' | b'e' | b'i' | b'o' | b'u' | b'v'))
        {
            continue;
        }
        for (corrected, dist) in get_spatial_typo_corrections(slice, 35) {
            let kind = classify_single_typo(slice, &corrected);
            let cost = dist.max(12);
            out.push(SyllableEdge {
                start,
                end: start + l,
                syllables: EdgeSyllables::from_one(&corrected),
                kind: EdgeKind::Typo(kind),
                typo_count: 1,
                cost,
            });
        }
        for corrected in get_typo_corrections_with_rules(slice, &options.typo_rules) {
            out.push(SyllableEdge {
                start,
                end: start + l,
                syllables: EdgeSyllables::from_one(&corrected),
                kind: EdgeKind::RuleNormalized,
                typo_count: 0,
                cost: 14,
            });
        }
    }

    // 1.5 单音节内相邻字符倒置（如 cehng -> cheng, xaing -> xiang）
    populate_single_syllable_transpose(start, seg, out);

    // 2. 跨音节倒置（如 zhognguo -> zhong'guo）
    populate_cross_syllable_transpose(start, seg, out);

    // 3. 边界共享尾字母漏按（如 zhonguo -> zhong'guo）
    populate_shared_boundary_missing(start, seg, out);
}

/// 根据源切片与目标音节特征判定单音节容错子类别
#[inline]
fn classify_single_typo(source: &str, target: &str) -> TypoKind {
    if source.len() == target.len() {
        let sb = source.as_bytes();
        let tb = target.as_bytes();
        for i in 0..sb.len().saturating_sub(1) {
            if sb[i] == tb[i + 1] && sb[i + 1] == tb[i] {
                return TypoKind::Transpose;
            }
        }
        let _ = weighted_edit_distance(source, target);
        TypoKind::Neighbor
    } else if source.len() < target.len() {
        TypoKind::Missing
    } else {
        TypoKind::Extra
    }
}

/// 探索单音节内相邻字符倒置（如 cehng -> cheng, xaing -> xiang）
fn populate_single_syllable_transpose(start: usize, seg: &str, out: &mut Vec<SyllableEdge>) {
    let seg_bytes = seg.as_bytes();
    let max_len = seg_bytes.len().min(6);
    if max_len < 3 {
        return;
    }
    let mut buf = [0u8; 6];
    for len in 3..=max_len {
        let slice = &seg[..len];
        if is_valid_syllable(slice) {
            continue;
        }
        buf[..len].copy_from_slice(&seg_bytes[..len]);
        for i in 0..len - 1 {
            if buf[i] == buf[i + 1] {
                continue;
            }
            buf.swap(i, i + 1);
            if let Ok(swapped) = std::str::from_utf8(&buf[..len])
                && is_valid_syllable(swapped)
            {
                out.push(SyllableEdge {
                    start,
                    end: start + len,
                    syllables: EdgeSyllables::from_one(swapped),
                    kind: EdgeKind::Typo(TypoKind::Transpose),
                    typo_count: 1,
                    cost: 16,
                });
            }
            buf.swap(i, i + 1);
        }
    }
}

/// 探索跨音节相邻字符倒置（如 zhognguo -> zhong'guo）
fn populate_cross_syllable_transpose(start: usize, seg: &str, out: &mut Vec<SyllableEdge>) {
    let seg_bytes = seg.as_bytes();
    let max_span = seg_bytes.len().min(12);
    if max_span < 4 {
        return;
    }

    let mut buf = [0u8; 12];
    for span in 4..=max_span {
        buf[..span].copy_from_slice(&seg_bytes[..span]);
        for i in 1..span - 1 {
            if buf[i] == buf[i + 1] {
                continue;
            }
            buf.swap(i, i + 1);
            if let Ok(swapped) = std::str::from_utf8(&buf[..span]) {
                for split in 2..=span.saturating_sub(2) {
                    if split > i + 1 {
                        break;
                    }
                    if split + 1 < i {
                        continue;
                    }
                    let (s1, s2) = swapped.split_at(split);
                    if is_valid_syllable(s1) && is_valid_syllable(s2) {
                        out.push(SyllableEdge {
                            start,
                            end: start + span,
                            syllables: EdgeSyllables::from_two(s1, s2),
                            kind: EdgeKind::Typo(TypoKind::Transpose),
                            typo_count: 1,
                            cost: 18,
                        });
                    }
                }
            }
            buf.swap(i, i + 1);
        }
    }
}

/// 探索边界共享尾字母漏按（如 zhonguo -> zhong + guo）
fn populate_shared_boundary_missing(start: usize, seg: &str, out: &mut Vec<SyllableEdge>) {
    let fsa = PrismFsa::global();
    for (syl1, mid) in fsa.match_syllables(seg, 0) {
        if mid < 2 || mid >= seg.len() {
            continue;
        }
        // 只对以 n/g 鼻音或常用尾音结尾的音节开启边界共享，避免无意义泛化
        let last_byte = seg.as_bytes()[mid - 1];
        if !matches!(last_byte, b'n' | b'g' | b'i' | b'o' | b'u') {
            continue;
        }
        let shared_slice = &seg[mid - 1..];
        for (syl2, rel_end2) in fsa.match_syllables(shared_slice, 0) {
            if rel_end2 < 2 {
                continue;
            }
            let end2 = start + (mid - 1) + rel_end2;
            out.push(SyllableEdge {
                start,
                end: end2,
                syllables: EdgeSyllables::from_two(syl1, syl2),
                kind: EdgeKind::Typo(TypoKind::Missing),
                typo_count: 1,
                cost: 22,
            });
        }
    }
}
