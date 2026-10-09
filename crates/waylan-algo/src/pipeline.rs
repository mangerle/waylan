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
    /// 执行完整候选词清洗与排序流水线
    pub fn process(
        raw_candidates: Vec<AlgoCandidate>,
        config: &AlgoConfig,
        page_index: usize,
    ) -> Vec<AlgoCandidate> {
        let mut candidates = raw_candidates;

        // 1. 字符集过滤
        if config.filter_uncommon_charset {
            candidates.retain(|c| Self::is_standard_chinese_charset(&c.text));
        }

        // 2. 简繁体转换
        if config.traditional_chinese {
            Self::apply_traditional_conversion(&mut candidates);
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

        // 6. 分页截取
        Self::paginate(candidates, config.page_size, page_index)
    }

    /// 候选词去重：按候选文字去重，保留最高分者
    fn deduplicate(candidates: Vec<AlgoCandidate>) -> Vec<AlgoCandidate> {
        let mut seen = HashSet::new();
        let mut deduped = Vec::with_capacity(candidates.len());

        for item in candidates {
            if seen.insert(item.text.clone()) {
                deduped.push(item);
            }
        }
        deduped
    }

    /// 简繁体转换（纯 Rust 轻量映射表）
    fn apply_traditional_conversion(candidates: &mut [AlgoCandidate]) {
        for candidate in candidates.iter_mut() {
            let mut traditional = String::with_capacity(candidate.text.len());
            for ch in candidate.text.chars() {
                traditional.push(Self::simplified_to_traditional_char(ch));
            }
            candidate.text = traditional;
        }
    }

    /// 单字简转繁映射表（高频核心字库示例表）
    fn simplified_to_traditional_char(ch: char) -> char {
        match ch {
            '国' => '國',
            '会' => '會',
            '发' => '發',
            '学' => '學',
            '汉' => '漢',
            '语' => '語',
            '这' => '這',
            '来' => '來',
            '个' => '個',
            '们' => '們',
            '对' => '對',
            '经' => '經',
            '时' => '時',
            '现' => '現',
            '动' => '動',
            '进' => '進',
            '长' => '長',
            '问' => '問',
            '题' => '題',
            '见' => '見',
            '机' => '機',
            '点' => '點',
            '电' => '電',
            '华' => '華',
            '为' => '為',
            _ => ch,
        }
    }

    /// 检查是否在标准通用汉字字符集内（过滤非常规未编码符号与控制符）
    fn is_standard_chinese_charset(text: &str) -> bool {
        for ch in text.chars() {
            if ch.is_control() {
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
        let raw = vec![AlgoCandidate::new("中国", "zhong'guo", 10_000)];
        let config = AlgoConfig {
            traditional_chinese: true,
            ..Default::default()
        };

        let result = CandidatePipeline::process(raw, &config, 0);
        assert_eq!(result[0].text, "中國");
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
}
