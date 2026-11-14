//! 维特比整句动态规划解码器 (Viterbi Beam Search Decoder)

use super::language_model::BigramLanguageModel;
use super::tail_expansion::{expand_remaining_tail, TailExpansionContext};
use crate::pipeline::is_rare_ext_char;
use crate::table::LexiconTable;
use crate::types::{AlgoCandidate, CandidateTier};
use crate::user_adaptive::UserAdaptiveModel;

/// 整句动态规划求解的输入参数对象
///
/// 承载维特比整句解码所需的音节切分序列、末尾残存拼音、词典表、上屏历史上下文与简拼扩展开关。
///
/// 设计原理：整句解码的入参天然构成同一语义组，聚合为参数对象可避免调用处参数错位，
/// 同时为后续扩展解码策略保留稳定的扩展点。
pub struct SentenceDecodeParams<'a> {
    /// 音节切分序列
    pub syllables: &'a [String],
    /// 末尾残存拼音（尚未输入完整的末尾片段）
    pub remaining: &'a str,
    /// 词典表（提供词条与二元转移权重）
    pub table: &'a LexiconTable,
    /// 上屏历史上下文（用于二元语言模型的上下文转移评分）
    pub prev_context: Option<&'a str>,
    /// 是否启用简拼尾部扩展
    pub enable_acronym: bool,
    /// 可选的用户自适应词对搭配模型（用于融合个人习惯搭配加分）
    pub user_model: Option<&'a UserAdaptiveModel>,
    /// 评估时间衰减的基准时间戳（秒级 UNIX 时间）
    pub eval_ts: u64,
}

/// 词格点求解推进上下文参数对象
pub(super) struct LatticeSolveParams<'a> {
    pub(super) syllables: &'a [String],
    pub(super) table: &'a LexiconTable,
    pub(super) has_bigram: bool,
    pub(super) ctx_str: &'a str,
    pub(super) user_model: Option<&'a UserAdaptiveModel>,
    pub(super) eval_ts: u64,
}

/// 单个构词条目的固有属性（与前驱束无关的部分）
struct EntryWordInfo<'a> {
    /// 构词文本
    text: &'a str,
    /// 构词文本在二元词表中的词 ID
    word_id: Option<u32>,
    /// 构词文本的字符数
    char_len: i64,
    /// 构词文本的一元对数似然得分
    freq_score: i64,
}

/// 单个构词条目的束转移评估上下文参数对象
struct EntryTransitionParams<'a, 'b> {
    /// 词格点求解上下文（音节表、词典表、上屏上下文与二元模型标记）
    lattice: &'a LatticeSolveParams<'b>,
    /// 前驱束集合
    prev_beam: &'a [ViterbiBeamNode],
    /// 待评估的词典条目（文本、原始词频、词 ID）
    entry: &'a (String, u32, Option<u32>),
    /// 当前步的起始索引
    start_idx: usize,
}

/// 最佳前驱转移结果
struct BestTransition {
    /// 前驱束下标
    prev_beam_idx: usize,
    /// 累计总得分
    total_score: i64,
}

/// 单个音节跨度的条目展开与束剪枝上下文参数对象
struct SpanBeamParams<'a, 'b> {
    /// 词格点求解上下文
    lattice: &'a LatticeSolveParams<'b>,
    /// 当前跨度命中的原始词典条目
    raw_entries: &'a [(String, u32, Option<u32>)],
    /// 前驱束集合
    prev_beam: &'a [ViterbiBeamNode],
    /// 当前步的起始索引
    start_idx: usize,
    /// 单次检索的原始条目上限
    raw_limit: usize,
    /// 束搜索保留宽度
    beam_width: usize,
}

/// 束搜索动态规划格点状态（引入反向指针拓扑与词 ID 缓存，彻底消除中间路径字符串克隆与重复查表）
#[derive(Debug, Clone)]
pub(super) struct ViterbiBeamNode {
    /// 当前步转移的构词文本
    pub(super) word: String,
    /// 当前词在 Bigram 词表中的整数 ID 缓存
    pub(super) last_word_id: Option<u32>,
    /// 前驱步在 dp 中的索引（0 表示虚拟根节点）
    pub(super) prev_step: usize,
    /// 前驱节点在 dp[prev_step] 束集合中的下标
    pub(super) prev_beam_idx: usize,
    /// 累计对数语言模型得分
    pub(super) total_score: i64,
}

/// 整句回溯提取的句首词段候选元数据
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentenceHeadSegment {
    /// 句首词段文本（如 `nihaozhongguo` 的句首段「你好」）
    pub text: String,
    /// 对应的拼音键串
    pub pinyin: String,
    /// 覆盖的音节数
    pub syllable_count: usize,
}

/// 沿反向指针逆向回溯拼装整句文本，仅在选出全局最优解时触发一次聚合
pub(super) fn backtrack_sentence(
    dp: &[Vec<ViterbiBeamNode>],
    mut step: usize,
    mut beam_idx: usize,
    tail_word: Option<&str>,
) -> String {
    let mut words = Vec::new();
    if let Some(tail) = tail_word {
        words.push(tail);
    }
    while step > 0 {
        let node = &dp[step][beam_idx];
        words.push(&node.word);
        step = node.prev_step;
        beam_idx = node.prev_beam_idx;
    }
    words.reverse();
    words.concat()
}

/// 维特比整句动态规划解码器 (Viterbi Beam Search Decoder)
///
/// # 设计原理
/// - 在构建的标准 `WordLattice` 拓扑上，利用 `BigramLanguageModel` 的转移边权；
/// - 通过束搜索（Top-8 保留）在微秒内求解全局联合似然最高的整句路径。
#[derive(Debug, Default)]
pub struct ViterbiDecoder;

impl ViterbiDecoder {
    /// 默认基础束搜索最大保留宽度
    pub const DEFAULT_BEAM_WIDTH: usize = 8;

    /// 依据音节切分序列长度自适应计算束搜索保留宽度
    ///
    /// # 算法策略
    /// - 短词短语 (音节数 <= 4): 束宽 8，保持微秒级超低延迟；
    /// - 中等短语 (音节数 5..=8): 束宽 12，平衡探索广度与计算量；
    /// - 长句 (音节数 9..=15): 束宽 16，保留更多弱局部假设以保障全局连贯性；
    /// - 超长句 (音节数 > 15): 束宽 20，彻底解决前序贪心激进剪枝导致的长句失步问题。
    pub fn dynamic_beam_width(syllable_count: usize) -> usize {
        if syllable_count <= 4 {
            Self::DEFAULT_BEAM_WIDTH
        } else if syllable_count <= 8 {
            12
        } else if syllable_count <= 15 {
            16
        } else {
            20
        }
    }

    /// 对给定的音节切分列表与末尾残存拼音进行整句动态规划求解
    pub fn decode_sentence(
        syllables: &[String],
        remaining: &str,
        table: &LexiconTable,
    ) -> Option<AlgoCandidate> {
        Self::decode_sentence_with_context(syllables, remaining, table, None)
    }

    /// 对给定的音节切分列表、末尾残存拼音与上屏历史上下文进行整句动态规划求解
    pub fn decode_sentence_with_context(
        syllables: &[String],
        remaining: &str,
        table: &LexiconTable,
        prev_context: Option<&str>,
    ) -> Option<AlgoCandidate> {
        Self::decode_sentence_with_context_options(&SentenceDecodeParams {
            syllables,
            remaining,
            table,
            prev_context,
            enable_acronym: true,
            user_model: None,
            eval_ts: 0,
        })
    }

    /// 对给定的音节序列进行整句动态规划求解，并可控制是否启用简拼尾部扩展
    pub fn decode_sentence_with_context_options(
        params: &SentenceDecodeParams<'_>,
    ) -> Option<AlgoCandidate> {
        let SentenceDecodeParams {
            syllables,
            remaining,
            table,
            prev_context,
            enable_acronym,
            user_model,
            eval_ts,
        } = *params;

        let n = syllables.len();
        if n == 0 && remaining.is_empty() {
            return None;
        }

        let has_bigram = table.compact_bigram.is_some();
        let ctx_str = prev_context.unwrap_or("");
        let lattice = LatticeSolveParams {
            syllables,
            table,
            has_bigram,
            ctx_str,
            user_model,
            eval_ts,
        };

        // 沿音节序列向右推进求解 Viterbi 最优路径
        let dp = Self::solve_lattice_dp(&lattice);

        // 末尾残缺音节与声母扩展（处理未完成键入态）
        if !remaining.is_empty() {
            if let Some(cand) = expand_remaining_tail(
                &TailExpansionContext {
                    syllables,
                    remaining,
                    table,
                    has_bigram,
                    prev_context,
                    enable_acronym,
                },
                &dp,
            ) {
                return Some(cand);
            }
        }

        // 若无 remaining 或扩展未命中，直接提取到达第 n 个音节的全局最优解
        Self::extract_best_sentence(&dp, n, syllables)
    }

    /// 求解最优整句候选的同时，沿音节边界提取用于局部上屏的句首词段列表
    pub fn decode_sentence_with_heads(
        params: &SentenceDecodeParams<'_>,
    ) -> (Option<AlgoCandidate>, Vec<SentenceHeadSegment>) {
        let SentenceDecodeParams {
            syllables,
            remaining,
            table,
            prev_context,
            enable_acronym,
            user_model,
            eval_ts,
        } = *params;

        let n = syllables.len();
        if n == 0 && remaining.is_empty() {
            return (None, Vec::new());
        }

        let has_bigram = table.compact_bigram.is_some();
        let ctx_str = prev_context.unwrap_or("");
        let lattice = LatticeSolveParams {
            syllables,
            table,
            has_bigram,
            ctx_str,
            user_model,
            eval_ts,
        };
        let dp = Self::solve_lattice_dp(&lattice);

        if !remaining.is_empty() {
            if let Some(cand) = expand_remaining_tail(
                &TailExpansionContext {
                    syllables,
                    remaining,
                    table,
                    has_bigram,
                    prev_context,
                    enable_acronym,
                },
                &dp,
            ) {
                return (Some(cand), Vec::new());
            }
        }

        let sentence = Self::extract_best_sentence(&dp, n, syllables);
        let heads = Self::extract_head_segments(&dp, n, syllables);
        (sentence, heads)
    }

    /// 从最优路径回溯提取句首词段（仅在整句由 2 个及以上词组成且音节数 >= 3 时生成）
    fn extract_head_segments(
        dp: &[Vec<ViterbiBeamNode>],
        n: usize,
        syllables: &[String],
    ) -> Vec<SentenceHeadSegment> {
        if n < 3 || dp.get(n).is_none_or(Vec::is_empty) {
            return Vec::new();
        }
        let mut segments: Vec<(&str, usize)> = Vec::new();
        let mut step = n;
        let mut beam_idx = 0usize;
        while step > 0 {
            let node = &dp[step][beam_idx];
            segments.push((node.word.as_str(), step));
            step = node.prev_step;
            beam_idx = node.prev_beam_idx;
        }
        if segments.len() < 2 {
            return Vec::new();
        }
        segments.reverse();

        let mut out = Vec::new();
        // 4 音节及以上优先给出最长句首前缀（去掉最后一个词），随后给出首词本身
        if segments.len() >= 3 {
            let long_idx = segments.len() - 2;
            let long_syl_count = segments[long_idx].1;
            let long_text: String = segments[..=long_idx].iter().map(|(w, _)| *w).collect();
            out.push(SentenceHeadSegment {
                text: long_text,
                pinyin: syllables[..long_syl_count].join("'"),
                syllable_count: long_syl_count,
            });
        }
        let (first_word, first_syl_count) = segments[0];
        if first_syl_count >= 1 && first_syl_count < n {
            let first_text = first_word.to_string();
            if !out.iter().any(|h| h.text == first_text) {
                out.push(SentenceHeadSegment {
                    text: first_text,
                    pinyin: syllables[..first_syl_count].join("'"),
                    syllable_count: first_syl_count,
                });
            }
        }
        out
    }

    /// 构建词格点状态容器并沿音节序列推进求解 Viterbi 最优路径
    fn solve_lattice_dp(lattice: &LatticeSolveParams<'_>) -> Vec<Vec<ViterbiBeamNode>> {
        let n = lattice.syllables.len();
        let ctx_word_id = if lattice.has_bigram && !lattice.ctx_str.is_empty() {
            lattice.table.get_word_id(lattice.ctx_str)
        } else {
            None
        };

        // dp[i] 保存消耗了前 i 个完整音节时的最优束状态集合
        let mut dp: Vec<Vec<ViterbiBeamNode>> = vec![Vec::new(); n + 1];

        // 虚拟起始根节点
        dp[0].push(ViterbiBeamNode {
            word: String::new(),
            last_word_id: ctx_word_id,
            prev_step: 0,
            prev_beam_idx: 0,
            total_score: 0,
        });

        // 沿音节序列向右推进求解 Viterbi 最优路径
        Self::solve_syllable_lattice(lattice, &mut dp);

        dp
    }

    /// 从词格点状态中提取到达第 n 个音节的全局最优整句候选
    fn extract_best_sentence(
        dp: &[Vec<ViterbiBeamNode>],
        n: usize,
        syllables: &[String],
    ) -> Option<AlgoCandidate> {
        if dp[n].is_empty() {
            return None;
        }

        let best = &dp[n][0];
        let full_text = backtrack_sentence(dp, n, 0, None);
        let pinyin_str = syllables.join("'");
        let positive_score = to_positive_score(best.total_score);

        Some(
            AlgoCandidate::new(full_text, pinyin_str, positive_score)
                .with_comment("整句预测")
                .with_tier(CandidateTier::SentenceViterbi),
        )
    }

    /// 查询当前音节切片在词典中的候选条目（支持单字声母兜底）
    fn lookup_lattice_entries(
        table: &LexiconTable,
        key: &str,
        span: usize,
        raw_limit: usize,
    ) -> Option<Vec<(String, u32, Option<u32>)>> {
        match table.get_top_raw_entries(key, raw_limit) {
            Some(entries) if !entries.is_empty() => Some(entries),
            _ => {
                if span == 1 {
                    table.get_initial_raw_entries(key)
                } else {
                    None
                }
            }
        }
    }

    /// 针对单个构词条目计算从前驱束集合的最佳转移对数得分并入队目标束
    fn evaluate_entry_transition(
        params: &EntryTransitionParams<'_, '_>,
        target_beam: &mut Vec<ViterbiBeamNode>,
    ) {
        let (text, raw_score, word_id_opt) = params.entry;
        // 非常用扩展 B+ 生僻单字不参与整句组词搜索
        if is_rare_ext_char(text) {
            return;
        }
        let freq_score = unigram_log_score(*raw_score);
        let curr_word_id = word_id_opt.or_else(|| {
            if params.lattice.has_bigram {
                params.lattice.table.get_word_id(text)
            } else {
                None
            }
        });
        let word = EntryWordInfo {
            text: text.as_str(),
            word_id: curr_word_id,
            char_len: text.chars().count() as i64,
            freq_score,
        };

        let Some(best) = Self::find_best_prev_transition(params, &word) else {
            return;
        };

        Self::upsert_beam_node(target_beam, &word, params, best);
    }

    /// 在前驱束集合中搜索累计总分最高的前驱状态，返回最佳转移结果
    fn find_best_prev_transition(
        params: &EntryTransitionParams<'_, '_>,
        word: &EntryWordInfo<'_>,
    ) -> Option<BestTransition> {
        let mut best_prev_idx = 0usize;
        let mut best_total = i64::MIN;

        for (prev_beam_idx, prev_node) in params.prev_beam.iter().enumerate() {
            let prev_word = if params.start_idx == 0 {
                params.lattice.ctx_str
            } else {
                prev_node.word.as_str()
            };
            let base_score = BigramLanguageModel::transition_score_cached(
                (prev_word, prev_node.last_word_id),
                (word.text, word.word_id, word.char_len),
                word.freq_score,
                params.lattice.table,
            );
            // 叠加用户历史词对搭配习惯加分（带 30 天半衰期衰减）
            let pair_boost = params.lattice.user_model.map_or(0, |um| {
                um.pair_boost_score(prev_word, word.text, params.lattice.eval_ts)
            });
            let trans_score = base_score.saturating_add(pair_boost);
            let candidate_total = prev_node.total_score.saturating_add(trans_score);
            if candidate_total > best_total {
                best_total = candidate_total;
                best_prev_idx = prev_beam_idx;
            }
        }

        if best_total > i64::MIN {
            Some(BestTransition {
                prev_beam_idx: best_prev_idx,
                total_score: best_total,
            })
        } else {
            None
        }
    }

    /// 将最佳转移结果写入目标束集合（命中同词则更新分值，否则新增束状态）
    fn upsert_beam_node(
        target_beam: &mut Vec<ViterbiBeamNode>,
        word: &EntryWordInfo<'_>,
        params: &EntryTransitionParams<'_, '_>,
        best: BestTransition,
    ) {
        if let Some(existing) = target_beam.iter_mut().find(|node| node.word == word.text) {
            if best.total_score > existing.total_score {
                existing.prev_step = params.start_idx;
                existing.prev_beam_idx = best.prev_beam_idx;
                existing.total_score = best.total_score;
            }
        } else {
            target_beam.push(ViterbiBeamNode {
                word: word.text.to_string(),
                last_word_id: word.word_id,
                prev_step: params.start_idx,
                prev_beam_idx: best.prev_beam_idx,
                total_score: best.total_score,
            });
        }
    }

    /// 遍历音节切分列表并在词格点拓扑上推进束搜索动态规划
    fn solve_syllable_lattice(params: &LatticeSolveParams<'_>, dp: &mut [Vec<ViterbiBeamNode>]) {
        let LatticeSolveParams {
            syllables, table, ..
        } = *params;
        let n = syllables.len();
        let beam_width = Self::dynamic_beam_width(n);
        let raw_limit = if n > 4 { 6 } else { 4 };

        // 预分配复用 key 拼接缓冲区，消除 span > 1 时的临时 String 分配
        let mut key_buf = String::with_capacity(32);

        for start_idx in 0..n {
            if dp[start_idx].is_empty() {
                continue;
            }

            let max_span = (n - start_idx).min(5);

            for span in 1..=max_span {
                let end_idx = start_idx + span;
                let sub_syls = &syllables[start_idx..end_idx];
                let key = Self::build_span_key(sub_syls, &mut key_buf);

                let Some(raw_entries) = Self::lookup_lattice_entries(table, key, span, raw_limit)
                else {
                    continue;
                };

                let (left_dp, right_dp) = dp.split_at_mut(end_idx);
                let prev_beam = &left_dp[start_idx];
                let target_beam = &mut right_dp[0];

                Self::expand_span_beam(
                    &SpanBeamParams {
                        lattice: params,
                        raw_entries: &raw_entries,
                        prev_beam,
                        start_idx,
                        raw_limit,
                        beam_width,
                    },
                    target_beam,
                );
            }
        }
    }

    /// 拼接指定音节跨度对应的词典查询键（跨度为 1 时直接借用单音节，避免额外分配）
    fn build_span_key<'a>(sub_syls: &'a [String], key_buf: &'a mut String) -> &'a str {
        if sub_syls.len() == 1 {
            return sub_syls[0].as_str();
        }
        key_buf.clear();
        for (i, syl) in sub_syls.iter().enumerate() {
            if i > 0 {
                key_buf.push('\'');
            }
            key_buf.push_str(syl);
        }
        key_buf
    }

    /// 将当前跨度的词典条目逐条评估入束，并按束宽剪枝保留高分状态
    fn expand_span_beam(params: &SpanBeamParams<'_, '_>, target_beam: &mut Vec<ViterbiBeamNode>) {
        for entry in params.raw_entries.iter().take(params.raw_limit) {
            Self::evaluate_entry_transition(
                &EntryTransitionParams {
                    lattice: params.lattice,
                    prev_beam: params.prev_beam,
                    entry,
                    start_idx: params.start_idx,
                },
                target_beam,
            );
        }

        // 束搜索剪枝：保留得分最高的 Top-K 个不同末端词状态（根据序列长度自适应确定 K）
        target_beam.sort_by_key(|state| std::cmp::Reverse(state.total_score));
        target_beam.truncate(params.beam_width);
    }
}

/// 将原始静态词频转换为与二元转移矩阵同量纲的对数似然得分
#[inline]
pub(super) fn unigram_log_score(raw_score: u32) -> i64 {
    ((raw_score as f64 + 1.0).ln() * 1_200.0) as i64
}

/// 将 Viterbi 路径累计对数似然安全映射为正整数候选分
#[inline]
pub(super) fn to_positive_score(total_score: i64) -> u32 {
    total_score
        .saturating_add(100_000)
        .clamp(1, u32::MAX as i64) as u32
}
