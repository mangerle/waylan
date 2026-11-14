//! 按字节偏移索引的统一拼音音节格 (Syllable Lattice)
//!
//! # 设计原理
//! - **实现初衷**：替代旧版 `PrismEngine` 的整路径 DFS 枚举，将全拼、模糊音、习惯拼写、
//!   四类容错（邻键/倒置/漏按/多按，含跨音节倒置与边界共享漏按）、全拼混简拼声母以及
//!   末尾残余半成品全部统一生成到按字节位置索引的局部有向边中。
//! - **核心优势**：构建复杂度与输入长度呈线性关系，彻底消除长句组合爆炸；
//!   不同切分与纠错路径在同一张格上竞争，天然支持长句多处容错（`max_typos = 2`）。
//! - **代价与局限**：单边最多携带两个音节（用于表达跨音节倒置与边界共享漏按复合边）。

use super::alias::get_habitual_alias_targets;
use super::fsa::PrismFsa;
use super::fuzzy::PrismFuzzy;
use super::lattice_typo::populate_typo_edges;
use crate::syllables::{has_strictly_longer_syllable, has_syllable_prefix, is_valid_syllable};
use crate::types::FuzzyConfig;
use waylan_core::config::TypoCorrectionRules;

/// 容错边细分类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypoKind {
    /// 邻键误触替换
    Neighbor,
    /// 相邻字母倒置（含音节内与跨音节倒置）
    Transpose,
    /// 漏按字母（含边界共享尾字母漏按，如 zhonguo -> zhong'guo）
    Missing,
    /// 多按字母（如 shuang -> shuaang）
    Extra,
    /// 规则驱动的软纠错（如韵母/声母习惯纠错）
    RuleTypo,
}

/// 音节格边的语言学类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// 标准全拼精确音节
    Exact,
    /// 模糊音扩展音节
    Fuzzy,
    /// 习惯音/叹词规范化映射（如 en -> n/ng）
    RuleNormalized,
    /// 四类容错纠错边
    Typo(TypoKind),
    /// 混简拼单声母边（用于任意位置的全拼混简拼展开）
    Initial,
    /// 仅位于输入末尾的残余半成品前缀边
    PartialTail,
}

/// 音节格上的单条局部有向边
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyllableEdge {
    /// 原始输入中的起始字节偏移
    pub start: usize,
    /// 原始输入中的结束字节偏移
    pub end: usize,
    /// 该边对应的目标标准音节序列（常规为 1 个，跨音节倒置或边界共享漏按为 2 个）
    pub syllables: Vec<String>,
    /// 边类别
    pub kind: EdgeKind,
    /// 本边消耗的纠错次数（正常/模糊/简拼/半成品为 0，单处容错为 1）
    pub typo_count: u8,
    /// 图边基础惩罚代价（越小越优）
    pub cost: u32,
}

/// 音节格构建配置选项
#[derive(Debug, Clone, Default)]
pub struct LatticeOptions {
    /// 模糊音规则配置
    pub fuzzy: FuzzyConfig,
    /// 纠错规则配置
    pub typo_rules: TypoCorrectionRules,
    /// 是否生成单字母混简拼边
    pub enable_mixed_jianpin: bool,
    /// 是否生成单音节/跨音节容错边
    pub enable_typo_edges: bool,
}

/// 按字节偏移组织的全量音节格
#[derive(Debug, Clone)]
pub struct SyllableLattice {
    /// 原始输入字符串
    pub raw: String,
    /// 原始输入总字节长度
    pub len: usize,
    /// 按起始字节偏移 `0..len` 索引的出边列表
    pub edges_from: Vec<Vec<SyllableEdge>>,
}

impl SyllableLattice {
    /// 构建统一音节格
    pub fn build(raw: &str, options: &LatticeOptions) -> Self {
        let len = raw.len();
        let mut edges_from = vec![Vec::new(); len];
        if len == 0 {
            return Self {
                raw: raw.to_string(),
                len: 0,
                edges_from,
            };
        }

        let bytes = raw.as_bytes();
        let mut start = 0;
        while start < len {
            if bytes[start] == b'\'' {
                start += 1;
                continue;
            }
            let seg_end = find_segment_end(bytes, start);
            let seg = &raw[start..seg_end];
            let has_prev_letter = start > 0 && bytes[start - 1].is_ascii_alphabetic();

            populate_exact_and_fuzzy_edges(
                raw,
                start,
                seg,
                has_prev_letter,
                options,
                &mut edges_from[start],
            );

            if options.enable_typo_edges {
                populate_typo_edges(start, seg, options, &mut edges_from[start]);
            }

            if options.enable_mixed_jianpin {
                populate_initial_edge(start, bytes[start], &mut edges_from[start]);
            }

            if seg_end == len {
                populate_partial_tail_edge(start, seg, &mut edges_from[start]);
            }

            sort_and_dedup_edges(&mut edges_from[start]);
            start += 1;
        }

        Self {
            raw: raw.to_string(),
            len,
            edges_from,
        }
    }

    /// 跳过从 `pos` 开始的连续显式隔音符 `'`，返回下一个有效字节位置
    #[inline]
    pub fn skip_separators(&self, mut pos: usize) -> usize {
        let bytes = self.raw.as_bytes();
        while pos < self.len && bytes[pos] == b'\'' {
            pos += 1;
        }
        pos
    }

    /// 提取从位置 0 出发的最优无容错（或低代价）格式化音节序列
    pub fn best_formatted_preedit(&self) -> String {
        if self.len == 0 {
            return String::new();
        }
        let mut dp_cost = vec![u32::MAX; self.len + 1];
        let mut dp_prev: Vec<Option<(usize, Vec<String>)>> = vec![None; self.len + 1];
        dp_cost[0] = 0;

        for pos in 0..self.len {
            let cur_cost = dp_cost[pos];
            if cur_cost == u32::MAX {
                continue;
            }
            let next_non_sep = self.skip_separators(pos);
            if next_non_sep > pos {
                if cur_cost < dp_cost[next_non_sep] {
                    dp_cost[next_non_sep] = cur_cost;
                    dp_prev[next_non_sep] = dp_prev[pos].clone();
                }
                continue;
            }
            for edge in &self.edges_from[pos] {
                if edge.kind == EdgeKind::Fuzzy {
                    continue;
                }
                let penalty = match edge.kind {
                    EdgeKind::Exact => edge.cost,
                    EdgeKind::RuleNormalized => edge.cost + 5,
                    EdgeKind::PartialTail => 25,
                    EdgeKind::Initial => 70,
                    EdgeKind::Typo(_) => edge.cost + 40,
                    EdgeKind::Fuzzy => continue,
                };
                let cand_cost = cur_cost.saturating_add(penalty);
                if cand_cost < dp_cost[edge.end] {
                    dp_cost[edge.end] = cand_cost;
                    let mut seq = dp_prev[pos]
                        .as_ref()
                        .map(|(_, s)| s.clone())
                        .unwrap_or_default();
                    if matches!(
                        edge.kind,
                        EdgeKind::Initial | EdgeKind::PartialTail | EdgeKind::Typo(_)
                    ) {
                        seq.push(self.raw[edge.start..edge.end].to_string());
                    } else {
                        seq.extend(edge.syllables.iter().cloned());
                    }
                    dp_prev[edge.end] = Some((pos, seq));
                }
            }
        }

        dp_prev[self.len]
            .as_ref()
            .map(|(_, seq)| seq.join("'"))
            .unwrap_or_else(|| self.raw.clone())
    }
}

/// 查找当前连续拼音段落结束位置（遇到 `'` 或非字母即停）
#[inline]
fn find_segment_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end] != b'\'' {
        end += 1;
    }
    end
}

/// 判断零声母音节是否是在撕裂前一个完整音节（如 ti'an -> tian, ti'ji'ao -> ti'jiao, ti'a'n -> tian）
#[inline]
fn is_syllable_split_borrowing(raw: &str, start: usize, syl: &str) -> bool {
    let mut buf = [0u8; 8];
    let raw_bytes = raw.as_bytes();
    let syl_bytes = syl.as_bytes();
    let syl_end = start + syl_bytes.len();
    if syl_bytes.len() > 4 || syl_end > raw_bytes.len() {
        return false;
    }
    let max_extra = 2.min(raw_bytes.len() - syl_end);
    for prev_len in 1..=4.min(start) {
        let prev = &raw[start - prev_len..start];
        if !prev.bytes().all(|b| b.is_ascii_lowercase()) || !is_valid_syllable(prev) {
            continue;
        }
        let base_total = prev_len + syl_bytes.len();
        if base_total > buf.len() {
            continue;
        }
        buf[..prev_len].copy_from_slice(prev.as_bytes());
        buf[prev_len..base_total].copy_from_slice(syl_bytes);
        for extra in 0..=max_extra {
            if extra > 0 && !raw_bytes[syl_end + extra - 1].is_ascii_lowercase() {
                break;
            }
            let total = base_total + extra;
            if total > buf.len() {
                break;
            }
            if extra > 0 {
                buf[total - 1] = raw_bytes[syl_end + extra - 1];
            }
            if let Ok(merged) = std::str::from_utf8(&buf[..total]) {
                if is_valid_syllable(merged) {
                    return true;
                }
            }
        }
    }
    false
}

/// 生成标准精确边、模糊音边、习惯拼写别名边
fn populate_exact_and_fuzzy_edges(
    raw: &str,
    start: usize,
    seg: &str,
    has_prev_letter: bool,
    options: &LatticeOptions,
    out: &mut Vec<SyllableEdge>,
) {
    let fsa_matches = PrismFsa::global().match_syllables(seg, 0);
    for (syl, rel_end) in fsa_matches {
        let end = start + rel_end;
        let borrowing_penalty = if has_prev_letter && PrismFuzzy::is_zero_initial_syllable(syl) {
            if is_syllable_split_borrowing(raw, start, syl) {
                240
            } else {
                20
            }
        } else {
            0
        };
        out.push(SyllableEdge {
            start,
            end,
            syllables: vec![syl.to_string()],
            kind: EdgeKind::Exact,
            typo_count: 0,
            cost: borrowing_penalty,
        });

        if options.fuzzy.has_active_rules() {
            for variant in PrismFuzzy::expand_fuzzy_syllables(syl, &options.fuzzy) {
                out.push(SyllableEdge {
                    start,
                    end,
                    syllables: vec![variant],
                    kind: EdgeKind::Fuzzy,
                    typo_count: 0,
                    cost: borrowing_penalty + 12,
                });
            }
        }

        if let Some((targets, alias_cost)) = get_habitual_alias_targets(syl) {
            for &target in targets {
                out.push(SyllableEdge {
                    start,
                    end,
                    syllables: vec![target.to_string()],
                    kind: EdgeKind::RuleNormalized,
                    typo_count: 0,
                    cost: borrowing_penalty + alias_cost,
                });
            }
        }
    }

    // 处理自身非标准音节但开启模糊音后合法的切片（如开启 l/n 后的特殊组合）
    if options.fuzzy.has_active_rules() {
        let max_len = seg.len().min(7);
        for l in 2..=max_len {
            let slice = &seg[..l];
            if !is_valid_syllable(slice) && PrismFuzzy::is_fuzzy_valid(slice, &options.fuzzy) {
                for variant in PrismFuzzy::expand_fuzzy_syllables(slice, &options.fuzzy) {
                    out.push(SyllableEdge {
                        start,
                        end: start + l,
                        syllables: vec![variant],
                        kind: EdgeKind::Fuzzy,
                        typo_count: 0,
                        cost: 12,
                    });
                }
            }
        }
    }
}

/// 生成单字母混简拼边（排除永远不作汉语拼音声母的 i/u/v，避免撕裂韵母结构）
fn populate_initial_edge(start: usize, byte: u8, out: &mut Vec<SyllableEdge>) {
    if !byte.is_ascii_lowercase() || matches!(byte, b'i' | b'u' | b'v') {
        return;
    }
    let ch = byte as char;
    let mut s = String::with_capacity(1);
    s.push(ch);
    out.push(SyllableEdge {
        start,
        end: start + 1,
        syllables: vec![s],
        kind: EdgeKind::Initial,
        typo_count: 0,
        cost: 160,
    });
}

/// 在输入末尾生成残余半成品前缀边（如 shux -> shu + x, niha -> ni + ha）
fn populate_partial_tail_edge(start: usize, tail: &str, out: &mut Vec<SyllableEdge>) {
    if tail.is_empty() || tail.len() > 5 {
        return;
    }
    if is_valid_syllable(tail) && !has_strictly_longer_syllable(tail) {
        return;
    }
    if has_syllable_prefix(tail) {
        out.push(SyllableEdge {
            start,
            end: start + tail.len(),
            syllables: vec![tail.to_string()],
            kind: EdgeKind::PartialTail,
            typo_count: 0,
            cost: 110,
        });
    }
}

/// 对同一位置出发的边按代价升序排序并对同目标音节序列去重保留最优边
fn sort_and_dedup_edges(edges: &mut Vec<SyllableEdge>) {
    edges.sort_by(|a, b| {
        a.cost
            .cmp(&b.cost)
            .then_with(|| a.typo_count.cmp(&b.typo_count))
            .then_with(|| b.end.cmp(&a.end))
    });

    let mut seen: Vec<(usize, Vec<String>, bool)> = Vec::with_capacity(edges.len());
    edges.retain(|e| {
        let is_partial = matches!(e.kind, EdgeKind::Initial | EdgeKind::PartialTail);
        let key = (e.end, e.syllables.clone(), is_partial);
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
}
