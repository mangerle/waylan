//! 离线量化评测与逐键探针基准模块
//!
//! # 设计原理
//! - **实现初衷**：为分词、束搜索解码、纠错与排序权重调整提供客观、可复现的量化评估基准，
//!   避免依靠主观体感调参导致召回退化。
//! - **核心能力**：
//!   1. 批量评测（`run_eval`）：统计 Top-1 / Top-3 / Top-10 命中率及单次求值耗时；
//!   2. 逐键探针（`run_probe`）：逐字符增量输入，检测期望词是否出现“中途出现又消失”的退化缺陷。

use std::collections::BTreeMap;
use std::time::Instant;

use crate::decoder::ScoringWeights;
use crate::engine::AlgoEngine;

/// 单条离线评测用例
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalCase {
    /// 用例所属场景分类（如 homophone / partial / jianpin / typo / sentence）
    pub category: String,
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

/// 分场景汇总统计
#[derive(Debug, Clone)]
pub struct CategoryStat {
    /// 场景分类名
    pub category: String,
    /// 该场景用例总数
    pub total: usize,
    /// Top-1 命中数
    pub top1_hits: usize,
    /// Top-3 命中数
    pub top3_hits: usize,
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
    /// 各场景分项命中统计
    pub category_stats: Vec<CategoryStat>,
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
/// 支持两种格式：
/// - 2 列：`输入<TAB>期望1|期望2`（默认分类 `"general"`）
/// - 3 列：`分类<TAB>输入<TAB>期望1|期望2`
///
/// 自动忽略空行与 `#` 注释行。
pub fn parse_eval_tsv(content: &str) -> Vec<EvalCase> {
    let mut cases = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').map(str::trim).collect();
        let (category, input, expected_raw) = match cols.as_slice() {
            [inp, exp] => ("general", *inp, *exp),
            [cat, inp, exp, ..] => (*cat, *inp, *exp),
            _ => continue,
        };
        let expected: Vec<String> = expected_raw
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        if !input.is_empty() && !expected.is_empty() {
            cases.push(EvalCase {
                category: category.to_string(),
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
    let mut cat_map: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();

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

        let entry = cat_map.entry(case.category.clone()).or_insert((0, 0, 0));
        entry.0 += 1;

        if matches!(best_rank, Some(1)) {
            top1_hits += 1;
            entry.1 += 1;
        }
        if matches!(best_rank, Some(r) if r <= 3) {
            top3_hits += 1;
            entry.2 += 1;
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

    let category_stats = cat_map
        .into_iter()
        .map(|(category, (total, c_top1, c_top3))| CategoryStat {
            category,
            total,
            top1_hits: c_top1,
            top3_hits: c_top3,
        })
        .collect();

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
        category_stats,
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

/// 离线权重训练参数
#[derive(Debug, Clone, Copy)]
pub struct TrainConfig {
    /// 训练迭代轮数
    pub epochs: usize,
    /// Softmax 温度系数（平滑分数尺度差异）
    pub temperature: f64,
    /// 权重步长缩放系数
    pub step_scale: f64,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            epochs: 4,
            temperature: 50_000.0,
            step_scale: 0.15,
        }
    }
}

/// 离线权重训练输出报告
#[derive(Debug, Clone)]
pub struct TrainReport {
    /// 训练前 Softmax 交叉熵损失
    pub initial_loss: f64,
    /// 训练后 Softmax 交叉熵损失
    pub final_loss: f64,
    /// 训练前 Top-1 命中数
    pub initial_top1: usize,
    /// 训练后 Top-1 命中数
    pub final_top1: usize,
    /// 搜索得到的最优权重参数
    pub best_weights: ScoringWeights,
}

/// 计算标注评测集上的平均 Softmax 交叉熵损失（目标词不在候选内则计为惩罚上限 8.0）
pub fn compute_softmax_loss(engine: &mut AlgoEngine, cases: &[EvalCase], temperature: f64) -> f64 {
    if cases.is_empty() {
        return 0.0;
    }
    let temp = temperature.max(1.0);
    let mut total_loss = 0.0f64;

    for case in cases {
        engine.reset();
        for ch in case.input.chars() {
            engine.push_char(ch);
        }
        let cands = engine.all_candidates();
        let top = &cands[..cands.len().min(16)];
        let Some(target_idx) = top
            .iter()
            .position(|c| case.expected.iter().any(|e| e == &c.text))
        else {
            total_loss += 8.0;
            continue;
        };

        let max_score = top
            .iter()
            .map(|c| c.score as f64 / temp)
            .fold(f64::NEG_INFINITY, f64::max);
        let sum_exp: f64 = top
            .iter()
            .map(|c| ((c.score as f64 / temp) - max_score).exp())
            .sum();
        let target_logit = top[target_idx].score as f64 / temp - max_score;
        let loss = (sum_exp.ln() - target_logit).min(8.0);
        total_loss += loss;
    }

    total_loss / cases.len() as f64
}

/// 基于标注集执行坐标下降 + Softmax 交叉熵拟合，自动搜索更优的 [`ScoringWeights`]
pub fn train_weights_softmax(
    engine: &mut AlgoEngine,
    cases: &[EvalCase],
    train_cfg: &TrainConfig,
) -> TrainReport {
    let initial_weights = engine.config.scoring_weights;
    let initial_loss = compute_softmax_loss(engine, cases, train_cfg.temperature);
    let initial_top1 = run_eval(engine, cases).top1_hits;

    let mut best_weights = initial_weights;
    let mut best_loss = initial_loss;
    let mut best_top1 = initial_top1;

    for _epoch in 0..train_cfg.epochs {
        let mut improved = false;
        for (dim, delta_base) in [(0usize, 1_500i64), (1, 1_500), (2, 15), (3, 20), (4, 150)] {
            let step = ((delta_base as f64) * (1.0 + train_cfg.step_scale)).round() as i64;
            for dir in [-1i64, 1i64] {
                let candidate = perturb_weight(best_weights, dim, step * dir);
                engine.config.scoring_weights = candidate;
                let loss = compute_softmax_loss(engine, cases, train_cfg.temperature);
                let top1 = run_eval(engine, cases).top1_hits;
                if loss + 1e-4 < best_loss || (top1 > best_top1 && loss <= best_loss + 0.05) {
                    best_loss = loss;
                    best_top1 = top1;
                    best_weights = candidate;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }

    engine.config.scoring_weights = best_weights;
    TrainReport {
        initial_loss,
        final_loss: best_loss,
        initial_top1,
        final_top1: best_top1,
        best_weights,
    }
}

fn perturb_weight(mut w: ScoringWeights, dim: usize, delta: i64) -> ScoringWeights {
    match dim {
        0 => w.base_word_penalty = (w.base_word_penalty + delta).clamp(2_000, 40_000),
        1 => w.length_bonus_per_char = (w.length_bonus_per_char + delta).clamp(2_000, 40_000),
        2 => {
            w.partial_extension_penalty =
                (w.partial_extension_penalty as i64 + delta).clamp(0, 500) as u32
        }
        3 => w.typo_cost_penalty = (w.typo_cost_penalty as i64 + delta).clamp(10, 500) as u32,
        _ => {
            w.user_boost_scale_permille =
                (w.user_boost_scale_permille as i64 + delta).clamp(100, 5_000) as u32
        }
    }
    w
}

/// 返回覆盖 5 大场景（同音词、半成品、简拼与混拼、四类纠错、长句）的内置回归基准用例集
pub fn builtin_regression_cases() -> Vec<EvalCase> {
    parse_eval_tsv(
        "\
homophone\tnihao\t你好
homophone\tzhongguo\t中国
homophone\tbeijing\t北京
homophone\tshanghai\t上海
homophone\txian\t先|西安
homophone\tzhongdi\t种地
partial\tniha\t你好
partial\tzhonggu\t中国
partial\tbeiji\t北京
partial\tshangha\t上海
partial\tzhongd\t重点|种地
jianpin\twanqky\t完全可以
jianpin\tyongzwen\t用中文
jianpin\tbj\t北京
jianpin\tsh\t上海|是
typo\tzhognguo\t中国
typo\tzhonguo\t中国
typo\tnihaoo\t你好
typo\tnihoa\t你好
typo\tcehnggong\t成功
sentence\tnihaozhongguo\t你好中国
sentence\twoxiangqubeijing\t我想去北京
sentence\tjintianhao\t今天好
sentence\twomendeshijie\t我们的世界
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

        let loss = compute_softmax_loss(&mut engine, &cases, 50_000.0);
        assert!(loss >= 0.0);
        let report = train_weights_softmax(&mut engine, &cases, &TrainConfig::default());
        assert!(report.final_loss <= report.initial_loss + 1e-6);
    }
}
