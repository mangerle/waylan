//! 束搜索英文免切混输边与兜底连通边注入策略
//!
//! # 设计原理
//! - **实现初衷**：将英文词库混输、高频技术短词优先提权、全拼防截胡判定与断链兜底边注入
//!   从核心束搜索推进逻辑中剥离，保持位置束搜索调度器的单一职责。
//! - **核心优势**：集中管理中英混输与容错兜底的启发式规则，支持独立演进与单元测试。
//! - **代价与局限**：极少数恰好可被全拼切分的真实英文词需通过技术词表或显式分隔符触发。

use std::sync::Arc;

use super::lattice::SyllableLattice;
use super::matcher::PhraseMatch;
use crate::custom_phrase::EnglishSuggestionManager;
use crate::syllables::{can_segment_into_syllables, is_valid_syllable};

/// 非合法汉语拼音的高频技术英文短词集合（免切中英混输场景给予优先权重）
const HIGH_FREQ_TECH_WORDS: &[&str] = &[
    "api", "bug", "cli", "cpu", "css", "dns", "docker", "git", "gpu", "html", "ide", "json", "jwt",
    "linux", "python", "rpc", "rust", "sdk", "sql", "ssh", "tcp", "tls", "toml", "udp", "url",
    "vim", "yaml",
];

/// 判断英文词条是否为高频技术短词（忽略大小写）
#[inline]
fn is_high_freq_tech_word(slice: &str) -> bool {
    HIGH_FREQ_TECH_WORDS
        .iter()
        .any(|w| w.eq_ignore_ascii_case(slice))
}

/// 不限长度的全拼可切分判定（`can_segment_into_syllables` 受 32 字节上限，无法覆盖整句输入）
fn can_segment_unbounded(s: &str) -> bool {
    if s.is_empty() || !s.is_ascii() {
        return false;
    }
    let n = s.len();
    let mut reachable = vec![false; n + 1];
    reachable[0] = true;
    for i in 0..n {
        if !reachable[i] {
            continue;
        }
        for len in 1..=(n - i).min(6) {
            if is_valid_syllable(&s[i..i + len]) {
                reachable[i + len] = true;
            }
        }
    }
    reachable[n]
}

/// 判断非技术短词的英文候选是否会误截胡可被中文完整切分的全拼区间
fn is_chinese_pinyin_hijack(raw: &str, pos: usize, end: usize) -> bool {
    if can_segment_into_syllables(&raw[pos..end]) {
        return true;
    }
    let run_end = raw[pos..]
        .bytes()
        .position(|b| !b.is_ascii_alphabetic())
        .map_or(raw.len(), |off| pos + off);
    end < run_end && can_segment_unbounded(&raw[pos..run_end])
}

/// 判断技术词候选是否只是整段纯拼音串中的一部分（如 `xiguapi` 中的 `api`、`meimeide` 中的 `ide`）
fn is_tech_word_inside_pinyin_run(raw: &str, pos: usize, end: usize) -> bool {
    let run_start = raw[..pos]
        .bytes()
        .rposition(|b| !b.is_ascii_alphabetic())
        .map_or(0, |i| i + 1);
    let run_end = raw[end..]
        .bytes()
        .position(|b| !b.is_ascii_alphabetic())
        .map_or(raw.len(), |off| end + off);
    if run_start == pos && run_end == end {
        return false;
    }
    can_segment_unbounded(&raw[run_start..run_end])
}

/// 构造合成词图边（用于英文混输边与兜底单字/片段边）
#[inline]
fn build_synthetic_match(
    pos: usize,
    end: usize,
    word: String,
    slice: &str,
    freq: u32,
    typo_count: u8,
    edge_cost: u32,
) -> PhraseMatch {
    PhraseMatch {
        start: pos,
        end,
        word,
        word_id: None,
        freq,
        matched_key: Arc::from(slice),
        syllable_count: 1,
        typo_count,
        edge_cost,
        has_initial: false,
        has_partial_tail: false,
        is_exact_all: false,
    }
}

/// 在词图当前位置注入高频英文词库边，实现免切中英混输（如 woxiangqugoogle -> 我想去google）
///
/// # 防截胡与提权策略
/// 1. 若同跨度已命中高频精确中文词（如 `pinyin` -> “拼音”），禁止英文边截胡中文；
/// 2. 对非合法拼音的高频技术短词（如 `bug`、`api`、`git`）给予零边惩罚与高频提权，避免被中文碎词吞没。
pub(super) fn append_english_lexicon_edges(
    lattice: &SyllableLattice,
    pos: usize,
    eng: &EnglishSuggestionManager,
    matches: &mut Vec<PhraseMatch>,
) {
    let hits = eng.match_exact_prefixes(&lattice.raw[pos..], 3, 24);
    let mut added_tech_end: Option<usize> = None;
    for (orig_word, eng_freq, byte_len) in hits {
        let end = pos + byte_len;
        let slice = &lattice.raw[pos..end];
        let is_tech = is_high_freq_tech_word(slice);
        if is_valid_syllable(slice)
            || (!is_tech && is_chinese_pinyin_hijack(&lattice.raw, pos, end))
            || (is_tech && is_tech_word_inside_pinyin_run(&lattice.raw, pos, end))
        {
            continue;
        }
        // 若同跨度已存在无容错、无简拼的高频精确中文词（如 pinyin -> 拼音），禁止英文边截胡
        let blocked_by_chinese = matches.iter().any(|m| {
            m.end == end && m.is_exact_all && m.typo_count == 0 && !m.has_initial && m.freq >= 500
        });
        if blocked_by_chinese {
            continue;
        }
        if is_tech {
            added_tech_end = Some(end);
        }
        let freq = if is_tech {
            eng_freq.max(80_000)
        } else {
            eng_freq.max(10_000)
        };
        let edge_cost = if is_tech { 0 } else { 12 };
        matches.push(build_synthetic_match(
            pos,
            end,
            orig_word.to_string(),
            slice,
            freq,
            0,
            edge_cost,
        ));
    }

    // 保底兜底：即使未加载外部 aux.dict，也确保内置高频技术短词（如 bug/api/git）可参与混输
    for &tech in HIGH_FREQ_TECH_WORDS {
        let tlen = tech.len();
        if pos + tlen > lattice.len {
            continue;
        }
        let end = pos + tlen;
        if added_tech_end == Some(end) {
            continue;
        }
        let slice = &lattice.raw[pos..end];
        if !slice.eq_ignore_ascii_case(tech)
            || is_tech_word_inside_pinyin_run(&lattice.raw, pos, end)
        {
            continue;
        }
        if matches
            .iter()
            .any(|m| m.end == end && m.word.eq_ignore_ascii_case(tech))
        {
            continue;
        }
        matches.push(build_synthetic_match(
            pos,
            end,
            tech.to_string(),
            slice,
            80_000,
            0,
            0,
        ));
    }
}

/// 在兜底连通解码模式下追加英文单词兜底边与单字符防断链边
pub(super) fn append_fallback_edges(
    lattice: &SyllableLattice,
    pos: usize,
    matches: &mut Vec<PhraseMatch>,
) {
    let raw = lattice.raw.as_bytes();
    let len = lattice.len;
    let mut seg_end = pos;
    while seg_end < len && raw[seg_end].is_ascii_alphabetic() {
        seg_end += 1;
    }
    let seg_len = seg_end.saturating_sub(pos);
    if (2..=12).contains(&seg_len) && (seg_end == len || raw[seg_end] == b'\'') {
        let seg = &lattice.raw[pos..seg_end];
        if !is_valid_syllable(seg) {
            matches.push(build_synthetic_match(
                pos,
                seg_end,
                seg.to_string(),
                seg,
                1,
                1,
                220,
            ));
        }
    }

    if !matches.iter().any(|m| m.end > pos) {
        let char_len = lattice.raw[pos..].chars().next().map_or(1, char::len_utf8);
        let next_pos = (pos + char_len).min(len);
        let ch_str = &lattice.raw[pos..next_pos];
        matches.push(build_synthetic_match(
            pos,
            next_pos,
            ch_str.to_string(),
            ch_str,
            1,
            1,
            350,
        ));
    }
}

#[cfg(test)]
mod hijack_tests {
    use super::{is_chinese_pinyin_hijack, is_tech_word_inside_pinyin_run};

    #[test]
    fn 可切分长区间应判定为截胡() {
        assert!(is_chinese_pinyin_hijack("xiguapi", 0, 5));
        assert!(is_chinese_pinyin_hijack("keyide", 0, 6));
        assert!(is_chinese_pinyin_hijack("mainage", 0, 4));
    }

    #[test]
    fn 非对齐碎片但整段可切分应判定为截胡() {
        assert!(is_chinese_pinyin_hijack("xiguapi", 0, 4));
    }

    #[test]
    fn 真实英文词不应被误拦() {
        assert!(!is_chinese_pinyin_hijack("google", 0, 6));
        assert!(!is_chinese_pinyin_hijack("googlewoai", 0, 6));
    }

    #[test]
    fn 技术词位于纯拼音长串内部应被拒绝() {
        let s = "xiguapilifangdongxileba";
        assert!(is_tech_word_inside_pinyin_run(s, 4, 7));
        let s = "wodasanyaxianzaihaoduomeimeidedifangya";
        assert!(is_tech_word_inside_pinyin_run(s, 27, 30));
    }

    #[test]
    fn 技术词独立或所在串无法读成拼音应保留() {
        assert!(!is_tech_word_inside_pinyin_run("bug", 0, 3));
        assert!(!is_tech_word_inside_pinyin_run("wodebugyouduo", 4, 7));
        assert!(!is_tech_word_inside_pinyin_run("woxihuanrust", 8, 12));
    }
}
