//! 按字节前缀冻结的增量束搜索解码缓存
//!
//! # 设计原理
//! - **实现初衷**：在连续逐键击键或末尾退格时，长句前端的音节切分与束搜索状态
//!   `beam[0..=safe_freeze]` 完全不受末尾新增/删除字符影响。本模块缓存上一次解码的
//!   位置束槽、`ArenaNode` 回溯池与 `spans_by_pos` 词匹配表，避免每次击键从头重算。
//! - **核心优势**：保持与全量 `PositionalBeamDecoder::decode` 数学等价（零准确率损失），
//!   同时兼容隔音符、退格修改与 6 音节跨边界长词补充扩展。
//! - **代价与局限**：仅加速共享前缀场景（追加与末尾退格）；中段插删或参数变更时自动回退全量重算。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::beam::{ArenaNode, BeamDecodeOptions, BeamState};
use super::lattice::SyllableLattice;
use super::matcher::PhraseMatch;

/// 增量缓存安全回退窗口（字节）：覆盖单音节最长全拼 + 容错字符 + 边界隔音符
const SAFE_SYLLABLE_BACKOFF_BYTES: usize = 8;

/// 单个 6 音节词组可能跨越的最大字节数（用于判断前置词匹配是否可能跨过冻结边界）
pub(super) const MAX_PHRASE_SPAN_BYTES: usize = 42;

/// 按位置同步的增量束搜索解码缓存
#[derive(Debug, Default, Clone)]
pub struct IncrementalBeamCache {
    /// 上一次解码的原始输入串
    pub(super) raw: String,
    /// 影响音节格与打分的配置指纹
    pub(super) config_sig: u64,
    /// 上一次解码使用的句首历史上下文词
    pub(super) last_commit_context: Option<String>,
    /// 上一次各字节位置剪枝后的束槽快照
    pub(super) beam_slots: Vec<Vec<BeamState>>,
    /// 上一次束搜索回溯节点池
    pub(super) node_arena: Vec<ArenaNode>,
    /// 记录推进到各位置 `pos` 之前时 `node_arena` 的长度，用于安全截断
    pub(super) arena_len_before_pos: Vec<usize>,
    /// 上一次各位置保留的词匹配列表
    pub(super) spans_by_pos: Vec<Vec<PhraseMatch>>,
    /// 上一次各位置 DAT 前缀探索触及的最远字节位置
    pub(super) pos_max_reach: Vec<usize>,
    /// 上一次构建的音节格（用于前缀追加时的增量复用）
    pub(super) last_lattice: Option<SyllableLattice>,
}

impl IncrementalBeamCache {
    /// 创建一个空的增量解码缓存
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取与指定配置签名匹配的缓存音节格
    #[inline]
    pub fn cached_lattice(&self, config_sig: u64) -> Option<&SyllableLattice> {
        (self.config_sig == config_sig)
            .then_some(self.last_lattice.as_ref())
            .flatten()
    }

    /// 保存最新构建的音节格供下一轮增量复用
    #[inline]
    pub fn store_lattice(&mut self, lattice: SyllableLattice) {
        self.last_lattice = Some(lattice);
    }

    /// 主动清空增量缓存（当用户自适应模型变更或重置时调用）
    pub fn invalidate(&mut self) {
        self.raw.clear();
        self.beam_slots.clear();
        self.node_arena.clear();
        self.arena_len_before_pos.clear();
        self.spans_by_pos.clear();
        self.pos_max_reach.clear();
        self.last_lattice = None;
    }

    /// 判定是否可复用前缀缓存，若命中则返回可安全冻结的字节边界 `freeze_pos`
    pub(super) fn compute_safe_freeze(
        &self,
        lattice: &SyllableLattice,
        options: &BeamDecodeOptions<'_>,
        config_sig: u64,
    ) -> Option<usize> {
        if self.raw.is_empty()
            || self.config_sig != config_sig
            || self.last_commit_context.as_deref() != options.last_commit_context
        {
            return None;
        }
        let lcp = common_prefix_bytes(&self.raw, &lattice.raw);
        let mut freeze = lcp.saturating_sub(SAFE_SYLLABLE_BACKOFF_BYTES);
        if freeze == 0 || freeze >= self.raw.len() || freeze >= lattice.len {
            return None;
        }
        if freeze >= self.beam_slots.len()
            || freeze >= self.arena_len_before_pos.len()
            || freeze >= self.spans_by_pos.len()
        {
            return None;
        }

        // 确保冻结边界不落在隔音符分隔区内部，向前微调至非隔音符起点
        let bytes = lattice.raw.as_bytes();
        while freeze > 0 && (bytes[freeze] == b'\'' || bytes[freeze - 1] == b'\'') {
            freeze -= 1;
        }
        if freeze == 0 || lattice.skip_separators(freeze) != freeze {
            return None;
        }

        Some(freeze)
    }

    /// 更新缓存快照以供下一轮击键复用
    pub(super) fn update(
        &mut self,
        raw: &str,
        config_sig: u64,
        last_commit_context: Option<&str>,
        snapshot: CacheSnapshot,
    ) {
        self.raw.clear();
        self.raw.push_str(raw);
        self.config_sig = config_sig;
        self.last_commit_context = last_commit_context.map(str::to_string);
        self.beam_slots = snapshot.beam_slots;
        self.node_arena = snapshot.node_arena;
        self.arena_len_before_pos = snapshot.arena_len_before_pos;
        self.spans_by_pos = snapshot.spans_by_pos;
        self.pos_max_reach = snapshot.pos_max_reach;
    }
}

/// 打包写入增量缓存的单次解码快照
pub(super) struct CacheSnapshot {
    pub beam_slots: Vec<Vec<BeamState>>,
    pub node_arena: Vec<ArenaNode>,
    pub arena_len_before_pos: Vec<usize>,
    pub spans_by_pos: Vec<Vec<PhraseMatch>>,
    pub pos_max_reach: Vec<usize>,
}

/// 计算 `LatticeOptions` 与 `BeamDecodeOptions` 影响解码结果的 64 位配置签名
pub fn compute_decode_config_sig(
    fuzzy_mask: u16,
    enable_typo_edges: bool,
    enable_mixed_jianpin: bool,
    options: &BeamDecodeOptions<'_>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    fuzzy_mask.hash(&mut hasher);
    enable_typo_edges.hash(&mut hasher);
    enable_mixed_jianpin.hash(&mut hasher);
    options.beam_width.hash(&mut hasher);
    options.max_typos.hash(&mut hasher);
    options.max_sentences.hash(&mut hasher);
    options.matcher_options.max_syllables.hash(&mut hasher);
    options.matcher_options.max_typos.hash(&mut hasher);
    options.matcher_options.top_k_per_key.hash(&mut hasher);
    options.matcher_options.max_initial_steps.hash(&mut hasher);
    options
        .matcher_options
        .filter_rare_ext_chars
        .hash(&mut hasher);
    options.weights.base_word_penalty.hash(&mut hasher);
    options.weights.length_bonus_per_char.hash(&mut hasher);
    options.weights.partial_extension_penalty.hash(&mut hasher);
    options.weights.typo_cost_penalty.hash(&mut hasher);
    options.weights.user_boost_scale_permille.hash(&mut hasher);
    hasher.finish()
}

/// 计算两个 UTF-8 字符串按字符边界对齐的最长公共前缀字节长度
fn common_prefix_bytes(a: &str, b: &str) -> usize {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let min_len = ab.len().min(bb.len());
    let mut i = 0;
    while i < min_len && ab[i] == bb[i] {
        i += 1;
    }
    while i > 0 && !a.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 将新匹配中跨过冻结边界 (`end > freeze_pos`) 的新增词条安全追加至 `slot` 末尾，
/// 严格保持已有 `0..old_len` 元素索引不变以免破坏 `ArenaNode.word_ref`
pub(super) fn merge_crossing_matches(
    slot: &mut Vec<PhraseMatch>,
    new_matches: Vec<PhraseMatch>,
    freeze_pos: usize,
) {
    let orig_len = slot.len();
    for m in new_matches {
        if m.end <= freeze_pos {
            continue;
        }
        let exists = slot[..orig_len]
            .iter()
            .any(|old| old.end == m.end && old.word == m.word && old.matched_key == m.matched_key);
        if !exists {
            slot.push(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::{LatticeOptions, PositionalBeamDecoder};
    use crate::table::LexiconTable;

    #[test]
    fn test_incremental_beam_cache_matches_full_decode_on_push_and_pop() {
        let table = LexiconTable::with_sample_data();
        let lattice_opts = LatticeOptions {
            enable_mixed_jianpin: true,
            enable_typo_edges: true,
            ..Default::default()
        };
        let beam_opts = BeamDecodeOptions::default();
        let config_sig = compute_decode_config_sig(0, true, true, &beam_opts);
        let mut cache = IncrementalBeamCache::new();

        let full_input = "beijingshijianwomen";
        // 逐步追加字符，每一步校验缓存结果与全量解码结果完全一致
        for end in 1..=full_input.len() {
            let prefix = &full_input[..end];
            let lattice = SyllableLattice::build(prefix, &lattice_opts);
            let expected = PositionalBeamDecoder::decode(&lattice, &table, None, &beam_opts);
            let actual = PositionalBeamDecoder::decode_with_cache(
                &lattice, &table, None, &beam_opts, &mut cache, config_sig,
            );
            assert_eq!(
                actual.sentences.first().map(|s| &s.text),
                expected.sentences.first().map(|s| &s.text),
                "追加前缀 '{}' 时首选词不一致",
                prefix
            );
            assert_eq!(
                actual.sentences.len(),
                expected.sentences.len(),
                "追加前缀 '{}' 时候选数不一致",
                prefix
            );
        }

        // 模拟退格回退，校验回退后结果依然与全量解码一致
        for end in (1..full_input.len()).rev() {
            let prefix = &full_input[..end];
            let lattice = SyllableLattice::build(prefix, &lattice_opts);
            let expected = PositionalBeamDecoder::decode(&lattice, &table, None, &beam_opts);
            let actual = PositionalBeamDecoder::decode_with_cache(
                &lattice, &table, None, &beam_opts, &mut cache, config_sig,
            );
            assert_eq!(
                actual.sentences.first().map(|s| &s.text),
                expected.sentences.first().map(|s| &s.text),
                "退格前缀 '{}' 时首选词不一致",
                prefix
            );
        }
    }
}
