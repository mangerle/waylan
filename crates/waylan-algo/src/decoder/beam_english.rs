//! 束搜索英文免切混输边与兜底连通边注入策略
//!
//! # 设计原理
//! - **实现初衷**：将英文词库混输、高频技术短词优先提权、全拼防截胡判定与断链兜底边注入
//!   从核心束搜索推进逻辑中剥离，保持位置束搜索调度器的单一职责。
//! - **核心优势**：集中管理中英混输与容错兜底的启发式规则；按连续字母段预计算可切分性，
//!   消除束搜索逐位置推进中的重复动态规划与堆分配。
//! - **代价与局限**：极少数恰好可被全拼切分的真实英文词需通过技术词表或显式分隔符触发。

use std::sync::Arc;

use super::lattice::SyllableLattice;
use super::matcher::PhraseMatch;
use crate::custom_phrase::EnglishSuggestionManager;
use crate::syllables::{
    can_segment_into_syllables, can_segment_into_syllables_unbounded, is_valid_syllable,
};

/// 非合法汉语拼音的高频技术英文短词集合（免切中英混输场景给予优先权重）
const HIGH_FREQ_TECH_WORDS: &[&str] = &[
    "api", "bug", "cli", "cpu", "css", "dns", "docker", "git", "gpu", "html", "ide", "json", "jwt",
    "linux", "python", "rpc", "rust", "sdk", "sql", "ssh", "tcp", "tls", "toml", "udp", "url",
    "vim", "yaml",
];

/// 编译期构建的高频技术短词首字母 ASCII 小写位掩码（O(1) 过滤非匹配起始位置）
const TECH_WORD_FIRST_BYTE_MASK: u32 = {
    let mut mask = 0u32;
    let mut i = 0;
    while i < HIGH_FREQ_TECH_WORDS.len() {
        let b = HIGH_FREQ_TECH_WORDS[i].as_bytes()[0];
        mask |= 1u32 << (b - b'a');
        i += 1;
    }
    mask
};

/// 连续 ASCII 字母段的全拼可切分预计算缓存
///
/// 在单次 `advance_beam` 开始前对输入一次性扫描各连续字母段 `[start, end)`，
/// 避免在 `for pos` 循环内对同一字母段重复执行全拼 DP 判定。
pub(super) struct AlphaRunPinyinCache {
    runs: Vec<AlphaRunEntry>,
}

struct AlphaRunEntry {
    start: usize,
    end: usize,
    is_full_pinyin: bool,
}

impl AlphaRunPinyinCache {
    /// 扫描输入字符串并预计算各连续 ASCII 字母段的全拼可切分性
    pub(super) fn new(raw: &str) -> Self {
        let bytes = raw.as_bytes();
        let mut runs = Vec::with_capacity(4);
        let mut i = 0;
        while i < bytes.len() {
            if !bytes[i].is_ascii_alphabetic() {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            let is_full_pinyin = can_segment_into_syllables_unbounded(&raw[start..i]);
            runs.push(AlphaRunEntry {
                start,
                end: i,
                is_full_pinyin,
            });
        }
        Self { runs }
    }

    /// 查找覆盖区间 `[pos, end)` 的连续字母段及其全拼可切分标志
    #[inline]
    fn find_run(&self, pos: usize, end: usize) -> Option<&AlphaRunEntry> {
        self.runs.iter().find(|r| r.start <= pos && end <= r.end)
    }
}

/// 判断英文词条是否为高频技术短词（忽略大小写）
#[inline]
fn is_high_freq_tech_word(slice: &str) -> bool {
    let Some(&first) = slice.as_bytes().first() else {
        return false;
    };
    let lower = first.to_ascii_lowercase();
    if !lower.is_ascii_lowercase() || (TECH_WORD_FIRST_BYTE_MASK & (1u32 << (lower - b'a'))) == 0 {
        return false;
    }
    HIGH_FREQ_TECH_WORDS
        .iter()
        .any(|w| w.eq_ignore_ascii_case(slice))
}

/// 判断非技术短词的英文候选是否会误截胡可被中文完整切分的全拼区间
#[inline]
fn is_chinese_pinyin_hijack(
    raw: &str,
    pos: usize,
    end: usize,
    run_cache: &AlphaRunPinyinCache,
) -> bool {
    if can_segment_into_syllables(&raw[pos..end]) {
        return true;
    }
    if let Some(run) = run_cache.find_run(pos, end) {
        return end < run.end
            && (pos == run.start && run.is_full_pinyin
                || (pos > run.start && can_segment_into_syllables_unbounded(&raw[pos..run.end])));
    }
    false
}

/// 判断技术词候选是否只是整段纯拼音串中的一部分（如 `xiguapi` 中的 `api`、`meimeide` 中的 `ide`）
#[inline]
fn is_tech_word_inside_pinyin_run(pos: usize, end: usize, run_cache: &AlphaRunPinyinCache) -> bool {
    let Some(run) = run_cache.find_run(pos, end) else {
        return false;
    };
    if run.start == pos && run.end == end {
        return false;
    }
    run.is_full_pinyin
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
        word_char_len: word.chars().count(),
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
    run_cache: &AlphaRunPinyinCache,
    matches: &mut Vec<PhraseMatch>,
) {
    let hits = eng.match_exact_prefixes(&lattice.raw[pos..], 3, 24);
    let mut added_tech_end: Option<usize> = None;
    for (orig_word, eng_freq, byte_len) in hits {
        let end = pos + byte_len;
        let slice = &lattice.raw[pos..end];
        let is_tech = is_high_freq_tech_word(slice);
        if is_valid_syllable(slice)
            || (!is_tech && is_chinese_pinyin_hijack(&lattice.raw, pos, end, run_cache))
            || (is_tech && is_tech_word_inside_pinyin_run(pos, end, run_cache))
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
    let Some(&first_byte) = lattice.raw.as_bytes().get(pos) else {
        return;
    };
    let lower_first = first_byte.to_ascii_lowercase();
    if !lower_first.is_ascii_lowercase()
        || (TECH_WORD_FIRST_BYTE_MASK & (1u32 << (lower_first - b'a'))) == 0
    {
        return;
    }

    for &tech in HIGH_FREQ_TECH_WORDS {
        if tech.as_bytes()[0] != lower_first {
            continue;
        }
        let tlen = tech.len();
        if pos + tlen > lattice.len {
            continue;
        }
        let end = pos + tlen;
        if added_tech_end == Some(end) {
            continue;
        }
        let slice = &lattice.raw[pos..end];
        if !slice.eq_ignore_ascii_case(tech) || is_tech_word_inside_pinyin_run(pos, end, run_cache)
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
    use super::{AlphaRunPinyinCache, is_chinese_pinyin_hijack, is_tech_word_inside_pinyin_run};

    #[test]
    fn 可切分长区间应判定为截胡() {
        let c1 = AlphaRunPinyinCache::new("xiguapi");
        assert!(is_chinese_pinyin_hijack("xiguapi", 0, 5, &c1));
        let c2 = AlphaRunPinyinCache::new("keyide");
        assert!(is_chinese_pinyin_hijack("keyide", 0, 6, &c2));
        let c3 = AlphaRunPinyinCache::new("mainage");
        assert!(is_chinese_pinyin_hijack("mainage", 0, 4, &c3));
    }

    #[test]
    fn 非对齐碎片但整段可切分应判定为截胡() {
        let c = AlphaRunPinyinCache::new("xiguapi");
        assert!(is_chinese_pinyin_hijack("xiguapi", 0, 4, &c));
    }

    #[test]
    fn 真实英文词不应被误拦() {
        let c1 = AlphaRunPinyinCache::new("google");
        assert!(!is_chinese_pinyin_hijack("google", 0, 6, &c1));
        let c2 = AlphaRunPinyinCache::new("googlewoai");
        assert!(!is_chinese_pinyin_hijack("googlewoai", 0, 6, &c2));
    }

    #[test]
    fn 技术词位于纯拼音长串内部应被拒绝() {
        let s = "xiguapilifangdongxileba";
        let c1 = AlphaRunPinyinCache::new(s);
        assert!(is_tech_word_inside_pinyin_run(4, 7, &c1));
        let s2 = "wodasanyaxianzaihaoduomeimeidedifangya";
        let c2 = AlphaRunPinyinCache::new(s2);
        assert!(is_tech_word_inside_pinyin_run(27, 30, &c2));
    }

    #[test]
    fn 技术词独立或所在串无法读成拼音应保留() {
        let c1 = AlphaRunPinyinCache::new("bug");
        assert!(!is_tech_word_inside_pinyin_run(0, 3, &c1));
        let c2 = AlphaRunPinyinCache::new("wodebugyouduo");
        assert!(!is_tech_word_inside_pinyin_run(4, 7, &c2));
        let c3 = AlphaRunPinyinCache::new("woxihuanrust");
        assert!(!is_tech_word_inside_pinyin_run(8, 12, &c3));
    }
}
