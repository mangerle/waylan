use crate::opencc::OpenCcDict;
use crate::types::{AlgoCandidate, AlgoConfig};
use std::collections::HashSet;

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

    /// 执行完整候选词清洗与排序流水线，返回全量排序列（不执行分页截断）
    pub fn process_all_with_opencc(
        raw_candidates: Vec<AlgoCandidate>,
        config: &AlgoConfig,
        opencc: Option<&OpenCcDict>,
    ) -> Vec<AlgoCandidate> {
        let mut candidates = raw_candidates;

        // 1. 字符集过滤
        if config.filter_uncommon_charset {
            candidates.retain(|c| Self::is_standard_chinese_charset(&c.text));
        }

        // 2. 简繁体转换（基于 STPhrases 正向最大匹配 FMM + STCharacters 单字表及一简多繁展开）
        if config.traditional_chinese {
            if let Some(dict) = opencc {
                dict.convert_candidates(&mut candidates);
            } else {
                OpenCcDict::global().convert_candidates(&mut candidates);
            }
        }

        // 3. 多级综合排序：先比对优先级层级 Tier（成词 > 用户自造 > 拼句 > 前缀），同层级比对综合 score
        candidates.sort_by(|a, b| b.tier.cmp(&a.tier).then_with(|| b.score.cmp(&a.score)));

        // 4. 候选词去重（在有序列表中保留最高优先级与最高分者）
        if config.auto_dedup {
            candidates = Self::deduplicate(candidates);
        }

        // 5. 拼音注释挂载
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
    fn decorate_comments(candidates: &mut [AlgoCandidate]) {
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
mod tests {
    use super::*;

    #[test]
    fn test_deduplicate() {
        let raw = vec![
            AlgoCandidate::new("中国", "zhong'guo", 10_000),
            AlgoCandidate::new("中国", "zhong'guo", 5_000),
            AlgoCandidate::new("中", "zhong", 8_000),
        ];
        let config = AlgoConfig::default();
        let result = CandidatePipeline::process(raw, &config, 0);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "中国");
        assert_eq!(result[0].score, 10_000);
    }

    #[test]
    fn test_traditional_conversion() {
        let raw = vec![
            AlgoCandidate::new("中国", "zhong'guo", 10_000),
            AlgoCandidate::new("头发", "tou'fa", 9_000),
            AlgoCandidate::new("发展", "fa'zhan", 8_500),
            AlgoCandidate::new("皇后", "huang'hou", 8_000),
            AlgoCandidate::new("发", "fa", 7_000),
        ];
        let config = AlgoConfig {
            traditional_chinese: true,
            ..Default::default()
        };

        let result = CandidatePipeline::process(raw, &config, 0);
        assert_eq!(result[0].text, "中國");
        assert_eq!(result[1].text, "頭髮");
        assert_eq!(result[2].text, "發展");
        assert_eq!(result[3].text, "皇后");
        assert_eq!(result[4].text, "發");
        assert_eq!(result[5].text, "髮");
    }

    #[test]
    fn test_pagination() {
        let raw: Vec<AlgoCandidate> = (0..20)
            .map(|i| AlgoCandidate::new(format!("词{}", i), "ci", 20 - i as u32))
            .collect();

        let config = AlgoConfig {
            page_size: 5,
            ..Default::default()
        };

        let page0 = CandidatePipeline::process(raw.clone(), &config, 0);
        assert_eq!(page0.len(), 5);
        assert_eq!(page0[0].text, "词0");

        let page1 = CandidatePipeline::process(raw, &config, 1);
        assert_eq!(page1.len(), 5);
        assert_eq!(page1[0].text, "词5");
    }

    #[test]
    fn test_filter_pua_and_replacement_chars_retains_cjk_extensions() {
        let raw = vec![
            AlgoCandidate::new("常用字", "chang'yong'zi", 10_000),
            AlgoCandidate::new("\u{20000}", "he", 9_000), // CJK 扩展 B 区生僻字，必须保留
            AlgoCandidate::new("\u{E000}", "pua", 8_000), // BMP 私用区，应过滤
            AlgoCandidate::new("\u{F0000}", "spua", 7_000), // 补充私用区 A，应过滤
            AlgoCandidate::new("乱码\u{FFFD}", "luan'ma", 6_000), // 替换字符，应过滤
        ];
        let config = AlgoConfig {
            filter_uncommon_charset: true,
            ..Default::default()
        };

        let result = CandidatePipeline::process(raw, &config, 0);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "常用字");
        assert_eq!(result[1].text, "\u{20000}");
    }

    #[test]
    fn test_deduplicate_preserves_order_and_highest_score() {
        let raw = vec![
            AlgoCandidate::new("测试", "ce'shi", 100),
            AlgoCandidate::new("其他", "qi'ta", 90),
            AlgoCandidate::new("测试", "ce'shi", 80), // 重复项，应被剔除
            AlgoCandidate::new("微澜", "wei'lan", 70),
        ];
        let deduped = CandidatePipeline::deduplicate(raw);
        assert_eq!(deduped.len(), 3);
        assert_eq!(deduped[0].text, "测试");
        assert_eq!(deduped[0].score, 100);
        assert_eq!(deduped[1].text, "其他");
        assert_eq!(deduped[2].text, "微澜");
    }
}
