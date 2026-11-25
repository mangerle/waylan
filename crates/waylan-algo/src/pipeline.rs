use crate::opencc::OpenCcDict;
use crate::syllables::can_segment_into_syllables;
use crate::types::{AlgoCandidate, AlgoConfig};
use std::collections::HashSet;
use waylan_core::candidate::CandidateTier;

/// 扩展 B 区以后在常见字体（Noto CJK、思源黑体等）中可正常渲染的通用规范扩展汉字
const COMMON_EXT_CHARS: &str = concat!(
    "𠅤𠙶𠳐𡎚𡐓𣗋𣲗𣲘𣸣𤆵𤧛𤩽𤫉𥔲𥕢𥖨𥻗𦈡𦒍𦙶𦝼𦭜𦰡𧿹𨐈𨙸𨚕𨟠𨭉𨱇𨱏𨱑𨱔𨺙𩽾𩾃𩾌𪟝𪣻𪤗",
    "𪨰𪨶𪩘𪾢𫄧𫄨𫄷𫄸𫇭𫌀𫍣𫍯𫍲𫍽𫐄𫐐𫐓𫑡𫓧𫓯𫓶𫓹𫔍𫔎𫔶𫖮𫖯𫖳𫗧𫗴𫘜𫘝𫘦𫘧𫘨𫘪𫘬𫚕𫚖𫚭",
    "𫛭𫞩𫟅𫟦𫟷𫟹𫟼𫠆𫠊𫠜𫢸𫫇𫭟𫭢𫭼𫮃𫰛𫵷𫶇𫷷𫸩𬀩𬀪𬂩𬃊𬇕𬇙𬇹𬉼𬊈𬊤𬌗𬍛𬍡𬍤𬒈𬒔𬒗𬕂𬘓",
    "𬘘𬘡𬘩𬘫𬘬𬘭𬘯𬙂𬙊𬙋𬜬𬜯𬞟𬟁𬟽𬣙𬣞𬣡𬣳𬤇𬤊𬤝𬨂𬨎𬩽𬪩𬬩𬬭𬬮𬬱𬬸𬬹𬬻𬬿𬭁𬭊𬭎𬭚𬭛𬭤",
    "𬭩𬭬𬭯𬭳𬭶𬭸𬭼𬮱𬮿𬯀𬯎𬱖𬱟𬳵𬳶𬳽𬳿𬴂𬴃𬴊𬶋𬶍𬶏𬶐𬶟𬶠𬶨𬶭𬶮𬷕𬸘𬸚𬸣𬸦𬸪𬹼𬺈𬺓𰻝",
);

/// 判断文本是否为应当默认屏蔽的非常用 CJK 扩展 B+ 生僻单字
///
/// 多字词组放行；单字若码位 >= `U+20000` 且未收录于 `COMMON_EXT_CHARS`，则判定为非常用生僻扩展字。
pub fn is_rare_ext_char(text: &str) -> bool {
    if text.len() != 4 || text.as_bytes()[0] < 0xF0 {
        return false;
    }
    let Some(ch) = text.chars().next() else {
        return false;
    };
    u32::from(ch) >= 0x20000 && !COMMON_EXT_CHARS.contains(ch)
}

/// 判断原始输入拼音串是否包含无法切分为合法音节的非法分段
///
/// # 判定规则
/// 1. 按隔音符号 `'` 分割为若干子段；
/// 2. 若存在任意非空子段无法被标准音节表完全切分（例如 `xai`），则判定为包含非法音节；
/// 3. 若所有子段均可被合法切分（例如 `yi`、`xia`），则判定为不含非法音节。
pub fn has_illegal_syllable(input: &str) -> bool {
    let clean = input.trim();
    if clean.is_empty() {
        return false;
    }
    if clean.contains('\'') {
        clean
            .split('\'')
            .filter(|seg| !seg.is_empty())
            .any(|seg| !can_segment_into_syllables(seg))
    } else {
        !can_segment_into_syllables(clean)
    }
}

/// 检查是否满足“非法音节 + 单次纠错明显领先混拼”的竞价提权条件
///
/// # 判定规则
/// 1. 原始输入必须显式包含非法音节；
/// 2. 候选列表中必须实际存在混拼候选（`SentenceViterbi` 且非纠错、带有非一致拼音注音）；
/// 3. 候选列表中必须存在单次纠错候选（`TypoCorrection` 且 `typo_count == 1`）；
/// 4. 单次纠错候选的最佳得分必须高于混拼候选的最佳得分。
fn should_boost_single_typo(candidates: &[AlgoCandidate], raw_input: Option<&str>) -> bool {
    let Some(input) = raw_input else {
        return false;
    };
    if !has_illegal_syllable(input) {
        return false;
    }

    // 提取单次纠错候选的最高分
    let best_typo = candidates
        .iter()
        .filter(|c| c.tier == CandidateTier::TypoCorrection && c.typo_count == 1)
        .map(|c| c.score)
        .max();

    let Some(typo_score) = best_typo else {
        return false;
    };

    // 提取混拼候选的最高分（必须为 SentenceViterbi 且非纠错、带有非一致拼音注音）
    let best_mixed = candidates
        .iter()
        .filter(|c| {
            c.tier == CandidateTier::SentenceViterbi && c.typo_count == 0 && c.comment.is_some()
        })
        .map(|c| c.score)
        .max();

    // 仅在真实存在混拼候选被压制时，才允许单次纠错按分数竞争；若无混拼候选（如纯简拼 bj）则不触发
    let Some(mixed_score) = best_mixed else {
        return false;
    };

    typo_score > mixed_score
}

/// 候选词排序器 (Candidate Ranker)
///
/// # 设计原理
/// - **实现初衷**：统一候选词分级排序与精确词首位保护策略，
///   消除零散的层级重写与事后切片挪动。
/// - **核心优势**：
///   1. 语义分明：将“单次纠错提权”与“首位精确保护（方案 A）”显式表达为内聚算法；
///   2. 保护首位：确保在存在精确匹配时，空格首选永远忠实于用户敲击的原始音节，
///      同时高频模糊音词进入 Top 2+ 前列参与高效选词；
///   3. 零堆分配：就地利用排序与元素定位，避免无谓的内存分配与克隆。
/// - **代价与局限**：在首位出现模糊音且存在原生精确词时，执行一次常数时间的就地右旋（`rotate_right(1)`）。
pub struct CandidateRanker;

impl CandidateRanker {
    /// 按有效层级与得分降序执行 $O(N)$ 部分选择，仅保留前 `limit` 个高价值候选
    pub fn partial_select_top(
        candidates: &mut Vec<AlgoCandidate>,
        enable_typo_boost: bool,
        limit: usize,
    ) {
        if limit == 0 {
            candidates.clear();
            return;
        }
        if candidates.len() <= limit {
            return;
        }
        candidates.select_nth_unstable_by(limit - 1, |a, b| {
            Self::effective_tier(b, enable_typo_boost)
                .cmp(&Self::effective_tier(a, enable_typo_boost))
                .then_with(|| b.score.cmp(&a.score))
        });
        candidates.truncate(limit);
    }

    /// 执行完整候选词分级排序与首位保护
    pub fn rank(candidates: &mut [AlgoCandidate], enable_typo_boost: bool) {
        if candidates.len() <= 1 {
            return;
        }

        // 1. 基于有效层级与综合得分降序排列
        candidates.sort_by(|a, b| {
            Self::effective_tier(b, enable_typo_boost)
                .cmp(&Self::effective_tier(a, enable_typo_boost))
                .then_with(|| b.score.cmp(&a.score))
        });

        // 2. 方案 A 首选保护：若排在首位的是模糊音/规则归一候选，且候选集中存在原生精确候选，
        //    则将最高分的精确候选提升至首位，后续候选保持既定得分降序。
        if let Some(first_cand) = candidates.first()
            && first_cand.tier == CandidateTier::RuleNormalization
            && let Some(exact_idx) = candidates.iter().position(|c| {
                matches!(
                    c.tier,
                    CandidateTier::ExactWord
                        | CandidateTier::UserHistory
                        | CandidateTier::ProtectedEntity
                )
            })
        {
            candidates[..=exact_idx].rotate_right(1);
        }
    }

    /// 计算候选的实际比较档位
    #[inline]
    fn effective_tier(cand: &AlgoCandidate, enable_typo_boost: bool) -> CandidateTier {
        if enable_typo_boost && cand.tier == CandidateTier::TypoCorrection && cand.typo_count == 1 {
            CandidateTier::SentenceViterbi
        } else if cand.tier == CandidateTier::RuleNormalization {
            // 模糊音与规则归一提升至与 ExactWord 同级竞技，由综合词频打分决定展现先后
            CandidateTier::ExactWord
        } else {
            cand.tier
        }
    }
}

/// 候选词后处理流水线 (Candidate Pipeline)
///
/// # 设计原理
/// - **实现初衷**：将检索与解码产出的原始候选词列表，经过一系列标准过滤器（去重、字符集限制、简繁转换、注释修饰与分页），
///   形成最终交付给 UI 渲染的展示候选。
/// - **核心优势**：基于标准 Rust 迭代器链式转换，零运行时反射开销，流水线阶段清晰可控。
/// - **代价与局限**：对长候选列表排序与去重会产生局部内存分配，但在分页阈值（通常 50 条以内）下极为极速。
#[derive(Debug, Default)]
pub struct CandidatePipeline;

impl CandidatePipeline {
    /// 执行完整候选词清洗与排序流水线（使用默认全局简繁词典）
    pub fn process(
        raw_candidates: Vec<AlgoCandidate>,
        config: &AlgoConfig,
        page_index: usize,
    ) -> Vec<AlgoCandidate> {
        Self::process_with_opencc(raw_candidates, config, page_index, None)
    }

    /// 执行候选词核心清洗、排序与去重流水线（不挂载拼音注释，供按页延迟装饰）
    pub fn process_core_with_opencc(
        raw_candidates: Vec<AlgoCandidate>,
        config: &AlgoConfig,
        opencc: Option<&OpenCcDict>,
    ) -> Vec<AlgoCandidate> {
        Self::process_core_with_context(raw_candidates, config, opencc, None)
    }

    /// 执行候选词核心清洗、排序与去重流水线（支持注入原始输入上下文用于非法音节提权判定）
    pub fn process_core_with_context(
        raw_candidates: Vec<AlgoCandidate>,
        config: &AlgoConfig,
        opencc: Option<&OpenCcDict>,
        raw_input: Option<&str>,
    ) -> Vec<AlgoCandidate> {
        let mut candidates = raw_candidates;

        // 1. 字符集过滤（先清除控制符与 PUA，再按需过滤无字体支持的扩展 B+ 非常用单字）
        if config.filter_uncommon_charset {
            candidates.retain(|c| Self::is_standard_chinese_charset(&c.text));
        }
        if config.filter_rare_ext_chars {
            candidates.retain(|c| !is_rare_ext_char(&c.text));
        }

        // 2. 预截断并限制进入简繁转换与注释装饰的最大候选窗口，避免短拼长尾候选触发无谓开销
        //    先以 O(N) 部分选择保留前 160 条高价值候选，避免对全部长尾候选执行 O(N log N) 全量排序
        const MAX_PRE_OPENCC_CANDIDATES: usize = 160;
        const MAX_PUBLISHED_CANDIDATES: usize = 120;
        let enable_typo_boost = should_boost_single_typo(&candidates, raw_input);
        CandidateRanker::partial_select_top(
            &mut candidates,
            enable_typo_boost,
            MAX_PRE_OPENCC_CANDIDATES,
        );

        // 3. 简繁体转换（基于 STPhrases 正向最大匹配 FMM + STCharacters 单字表及一简多繁展开）
        if config.traditional_chinese {
            if let Some(dict) = opencc {
                dict.convert_candidates(&mut candidates);
            } else {
                OpenCcDict::global().convert_candidates(&mut candidates);
            }
        }
        CandidateRanker::rank(&mut candidates, enable_typo_boost);

        // 4. 候选词去重（在有序列表中保留最高优先级与最高分者）
        if config.auto_dedup {
            candidates = Self::deduplicate(candidates);
        }
        if candidates.len() > MAX_PUBLISHED_CANDIDATES {
            candidates.truncate(MAX_PUBLISHED_CANDIDATES);
        }
        Self::materialize_deferred_comments(&mut candidates);

        candidates
    }

    /// 为通过延迟标记（空字符串哨兵）保留的候选词统一格式化 `(pinyin)` 圆括号注释
    fn materialize_deferred_comments(candidates: &mut [AlgoCandidate]) {
        for cand in candidates.iter_mut() {
            if let Some(ref mut comment) = cand.comment
                && comment.is_empty()
                && !cand.pinyin.is_empty()
            {
                comment.reserve(cand.pinyin.len() + 2);
                comment.push('(');
                comment.push_str(&cand.pinyin);
                comment.push(')');
            }
        }
    }

    /// 执行完整候选词清洗与排序流水线，返回全量排序列（不执行分页截断）
    pub fn process_all_with_opencc(
        raw_candidates: Vec<AlgoCandidate>,
        config: &AlgoConfig,
        opencc: Option<&OpenCcDict>,
    ) -> Vec<AlgoCandidate> {
        Self::process_all_with_context(raw_candidates, config, opencc, None)
    }

    /// 执行完整候选词清洗与排序流水线（支持注入原始输入上下文）
    pub fn process_all_with_context(
        raw_candidates: Vec<AlgoCandidate>,
        config: &AlgoConfig,
        opencc: Option<&OpenCcDict>,
        raw_input: Option<&str>,
    ) -> Vec<AlgoCandidate> {
        let mut candidates =
            Self::process_core_with_context(raw_candidates, config, opencc, raw_input);

        // 5. 拼音注释挂载（仅对截断保留的有效候选窗口执行格式化）
        if config.show_pinyin_comment {
            Self::decorate_comments(&mut candidates);
        }

        candidates
    }

    /// 执行完整候选词清洗与排序流水线（支持注入自定义简繁转换词典实例并进行单页截取）
    pub fn process_with_opencc(
        raw_candidates: Vec<AlgoCandidate>,
        config: &AlgoConfig,
        page_index: usize,
        opencc: Option<&OpenCcDict>,
    ) -> Vec<AlgoCandidate> {
        let candidates = Self::process_all_with_opencc(raw_candidates, config, opencc);

        // 6. 分页截取
        Self::paginate(candidates, config.page_size, page_index)
    }

    /// 候选词去重：按候选文字去重，保留最高分者（基于借用切片与就地保留消除无谓堆内存分配）
    fn deduplicate(mut candidates: Vec<AlgoCandidate>) -> Vec<AlgoCandidate> {
        let mut seen = HashSet::with_capacity(candidates.len());
        let mut keep = Vec::with_capacity(candidates.len());
        for item in &candidates {
            keep.push(seen.insert(item.text.as_str()));
        }
        drop(seen);

        let mut keep_iter = keep.into_iter();
        candidates.retain(|_| keep_iter.next().unwrap_or(false));
        candidates
    }

    /// 检查是否在标准通用汉字字符集内（过滤控制符、替换字符及 Unicode 私用区 PUA 字符，保留 CJK 扩展区汉字）
    fn is_standard_chinese_charset(text: &str) -> bool {
        for ch in text.chars() {
            if ch.is_control()
                || ch == '\u{FFFD}'
                || matches!(
                    ch,
                    '\u{E000}'..='\u{F8FF}'
                        | '\u{F0000}'..='\u{FFFFD}'
                        | '\u{100000}'..='\u{10FFFD}'
                )
            {
                return false;
            }
        }
        true
    }

    /// 挂载拼音注释
    pub(crate) fn decorate_comments(candidates: &mut [AlgoCandidate]) {
        for candidate in candidates.iter_mut() {
            if candidate.comment.is_none() && !candidate.pinyin.is_empty() {
                candidate.comment = Some(format!("[{}]", candidate.pinyin));
            }
        }
    }

    /// 分页提取
    fn paginate(
        candidates: Vec<AlgoCandidate>,
        page_size: usize,
        page_index: usize,
    ) -> Vec<AlgoCandidate> {
        let size = if page_size == 0 { 7 } else { page_size };
        let start = page_index.saturating_mul(size);
        if start >= candidates.len() {
            return Vec::new();
        }
        candidates.into_iter().skip(start).take(size).collect()
    }
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
