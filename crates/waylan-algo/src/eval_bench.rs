//! 离线量化评测与逐键探针基准模块
//!
//! # 设计原理
//! - **实现初衷**：为分词、束搜索解码、纠错与排序权重调整提供客观、可复现的量化评估基准，
//!   避免依靠主观体感调参导致召回退化。
//! - **核心能力**：
//!   1. 批量评测（`run_eval`）：统计 Top-1 / Top-3 / Top-10 命中率及单次求值耗时；
//!   2. 逐键探针（`run_probe`）：逐字符增量输入，检测期望词是否出现“中途出现又消失”的退化缺陷。

use std::time::Instant;

use crate::engine::AlgoEngine;

/// 单条离线评测用例
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalCase {
    /// 原始按键输入序列（如 `"nihao"`）
    pub input: String,
    /// 可接受的期望候选词列表（任一命中即算成功）
    pub expected: Vec<String>,
}

/// 单条用例的评测明细结果
#[derive(Debug, Clone)]
pub struct EvalCaseResult {
    /// 原始输入
    pub input: String,
    /// 期望候选列表
    pub expected: Vec<String>,
    /// 命中的最佳排名（从 1 开始；未命中为 `None`）
    pub best_rank: Option<usize>,
    /// 引擎实际返回的首选词
    pub top1_text: String,
    /// 本次求值耗时（微秒）
    pub elapsed_us: u64,
}

/// 批量评测汇总报告
#[derive(Debug, Clone)]
pub struct EvalSummary {
    /// 总用例数
    pub total: usize,
    /// Top-1 命中数
    pub top1_hits: usize,
    /// Top-3 命中数
    pub top3_hits: usize,
    /// Top-10 命中数
    pub top10_hits: usize,
    /// 平均耗时（微秒）
    pub avg_us: f64,
    /// P99 耗时（微秒）
    pub p99_us: u64,
    /// 逐条未进 Top-3 的失败用例明细
    pub misses: Vec<EvalCaseResult>,
}

/// 逐键探针单步状态
#[derive(Debug, Clone)]
pub struct ProbeStep {
    /// 当前输入前缀
    pub prefix: String,
    /// 期望词在该前缀下的排名（从 1 开始；不在前 `max_check` 中为 `None`）
    pub rank: Option<usize>,
    /// 当前前缀下的首位候选文本
    pub top1_text: String,
}

/// 单条用例的逐键探针结果
#[derive(Debug, Clone)]
pub struct ProbeResult {
    /// 完整输入
    pub input: String,
    /// 目标期望词
    pub expected: Vec<String>,
    /// 是否出现“曾经命中但后续更长前缀时消失”的退化现象
    pub disappeared: bool,
    /// 各前缀步明细
    pub steps: Vec<ProbeStep>,
}

/// 从 TSV 文本解析评测用例
///
/// 格式要求：`输入<TAB>期望1|期望2`，忽略空行与 `#` 注释行。
pub fn parse_eval_tsv(content: &str) -> Vec<EvalCase> {
    let mut cases = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.splitn(2, '\t');
        let Some(input) = parts.next().map(str::trim) else {
            continue;
        };
        let Some(expected_raw) = parts.next().map(str::trim) else {
            continue;
        };
        let expected: Vec<String> = expected_raw
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        if !input.is_empty() && !expected.is_empty() {
            cases.push(EvalCase {
                input: input.to_string(),
                expected,
            });
        }
    }
    cases
}

/// 批量运行离线准确率与延迟评测
pub fn run_eval(engine: &mut AlgoEngine, cases: &[EvalCase]) -> EvalSummary {
    let mut top1_hits = 0usize;
    let mut top3_hits = 0usize;
    let mut top10_hits = 0usize;
    let mut latencies_us = Vec::with_capacity(cases.len());
    let mut misses = Vec::new();

    for case in cases {
        let start = Instant::now();
        let candidates = evaluate_input(engine, &case.input);
        let elapsed_us = start.elapsed().as_micros() as u64;
        latencies_us.push(elapsed_us);

        let best_rank = candidates
            .iter()
            .position(|text| case.expected.iter().any(|exp| exp == text))
            .map(|idx| idx + 1);
        let top1_text = candidates.first().cloned().unwrap_or_default();

        if matches!(best_rank, Some(1)) {
            top1_hits += 1;
        }
        if matches!(best_rank, Some(r) if r <= 3) {
            top3_hits += 1;
        } else {
            misses.push(EvalCaseResult {
                input: case.input.clone(),
                expected: case.expected.clone(),
                best_rank,
                top1_text,
                elapsed_us,
            });
        }
        if matches!(best_rank, Some(r) if r <= 10) {
            top10_hits += 1;
        }
    }

    let total = cases.len();
    let total_us: u64 = latencies_us.iter().copied().sum();
    let avg_us = if total == 0 {
        0.0
    } else {
        total_us as f64 / total as f64
    };
    latencies_us.sort_unstable();
    let p99_us = if latencies_us.is_empty() {
        0
    } else {
        let idx = ((latencies_us.len() - 1) * 99) / 100;
        latencies_us[idx]
    };

    EvalSummary {
        total,
        top1_hits,
        top3_hits,
        top10_hits,
        avg_us,
        p99_us,
        misses,
    }
}

/// 运行逐键探针，检测目标词在输入过程中是否出现过又消失
pub fn run_probe(
    engine: &mut AlgoEngine,
    cases: &[EvalCase],
    max_check: usize,
) -> Vec<ProbeResult> {
    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        let chars: Vec<char> = case.input.chars().collect();
        let mut steps = Vec::new();
        let mut seen_before = false;
        let mut disappeared = false;

        let start_len = if chars.len() >= 2 { 2 } else { 1 };
        for end in start_len..=chars.len() {
            let prefix: String = chars[..end].iter().collect();
            let cands = evaluate_input(engine, &prefix);
            let rank = cands
                .iter()
                .take(max_check)
                .position(|text| case.expected.iter().any(|exp| exp == text))
                .map(|idx| idx + 1);
            if rank.is_some() {
                seen_before = true;
            } else if seen_before {
                disappeared = true;
            }
            steps.push(ProbeStep {
                prefix,
                rank,
                top1_text: cands.first().cloned().unwrap_or_default(),
            });
        }

        results.push(ProbeResult {
            input: case.input.clone(),
            expected: case.expected.clone(),
            disappeared,
            steps,
        });
    }
    results
}

/// 将完整输入逐字送入引擎并返回全部候选文本
fn evaluate_input(engine: &mut AlgoEngine, input: &str) -> Vec<String> {
    engine.reset();
    for ch in input.chars() {
        engine.push_char(ch);
    }
    engine
        .all_candidates()
        .iter()
        .map(|cand| cand.text.clone())
        .collect()
}

/// 返回内置的通用回归基准用例集
pub fn builtin_regression_cases() -> Vec<EvalCase> {
    parse_eval_tsv(
        "\
nihao\t你好
zhongguo\t中国
beijing\t北京
shanghai\t上海
xian\t先|西安
zhongdi\t种地
nihaozhongguo\t你好中国
woxiangqubeijing\t我想去北京
jintianhao\t今天好
wanqky\t完全可以
yongzwen\t用中文
zhognguo\t中国
zhonguo\t中国
nihaoo\t你好
",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::LexiconTable;
    use crate::types::AlgoConfig;

    #[test]
    fn test_parse_eval_tsv_and_run_eval() {
        let tsv = "\
# 注释行
nihao\t你好
beijing\t北京|背景
";
        let cases = parse_eval_tsv(tsv);
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[1].expected, vec!["北京", "背景"]);

        let mut table = LexiconTable::new();
        table.insert("ni'hao", "你好", 5000);
        table.insert("bei'jing", "北京", 5000);
        let mut engine = AlgoEngine::new(table, AlgoConfig::default());

        let summary = run_eval(&mut engine, &cases);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.top1_hits, 2);
        assert_eq!(summary.top3_hits, 2);
        assert!(summary.misses.is_empty());

        let probes = run_probe(&mut engine, &cases, 10);
        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0].steps.len(), 4);
        assert_eq!(probes[0].steps.last().and_then(|s| s.rank), Some(1));
    }
}
