//! 微澜离线量化评测与逐键探针命令行工具

use std::env;
use std::fs;
use std::path::Path;
use std::time::Instant;

use waylan_algo::eval_bench::{
    builtin_regression_cases, parse_eval_tsv, run_eval, run_probe, train_weights_softmax, EvalCase,
    TrainConfig,
};
use waylan_algo::{AlgoConfig, AlgoEngine, CompactMmapBigram, LexiconTable, ScoringWeights};

fn arg_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|pos| args.get(pos + 1))
        .map(String::as_str)
}

fn load_cases_from_args(args: &[String]) -> Vec<EvalCase> {
    if let Some(path) = arg_value(args, "--cases") {
        if let Ok(content) = fs::read_to_string(path) {
            return parse_eval_tsv(&content);
        }
    }
    builtin_regression_cases()
}

fn load_engine(args: &[String]) -> Result<AlgoEngine, String> {
    let base_path = Path::new("data/base.dict.bin");
    let bigram_path = Path::new("data/bigram.table.bin");
    if !base_path.exists() {
        return Err("未找到 data/base.dict.bin，请在仓库根目录运行".to_string());
    }
    let t0 = Instant::now();
    let mut table =
        LexiconTable::load_binary(base_path).map_err(|e| format!("打开主词典失败: {e}"))?;
    if bigram_path.exists() {
        let bigram =
            CompactMmapBigram::open(bigram_path).map_err(|e| format!("打开 Bigram 失败: {e}"))?;
        table.compact_bigram = Some(bigram);
    }
    println!(
        "词库加载耗时: {:.2} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    let mut cfg = AlgoConfig::default();
    if let Some(wpath) = arg_value(args, "--weights") {
        let content = fs::read_to_string(wpath).map_err(|e| format!("读取权重文件失败: {e}"))?;
        let weights = ScoringWeights::from_kv_str(&content)?;
        cfg.scoring_weights = weights;
    }
    Ok(AlgoEngine::new(table, cfg))
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let probe_mode = args.iter().any(|a| a == "--probe");
    let train_mode = args.iter().any(|a| a == "--train");
    let cases = load_cases_from_args(&args);

    let mut engine = match load_engine(&args) {
        Ok(e) => e,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };

    if train_mode {
        let report = train_weights_softmax(&mut engine, &cases, &TrainConfig::default());
        println!(
            "Softmax 权重训练完成: loss {:.4} -> {:.4}, Top-1 {}/{} -> {}/{}",
            report.initial_loss,
            report.final_loss,
            report.initial_top1,
            cases.len(),
            report.final_top1,
            cases.len()
        );
        let kv_text = report.best_weights.to_kv_string();
        println!("最优权重:\n{kv_text}");
        if let Some(out_path) = arg_value(&args, "--weights-out") {
            if let Err(e) = fs::write(out_path, kv_text) {
                eprintln!("写入权重文件失败: {e}");
            } else {
                println!("已保存权重至 {out_path}");
            }
        }
        return;
    }

    if probe_mode {
        let probes = run_probe(&mut engine, &cases, 10);
        let disappeared_count = probes.iter().filter(|p| p.disappeared).count();
        println!(
            "逐键探针完成: 共 {} 条, 出现中途消失退化 {} 条",
            probes.len(),
            disappeared_count
        );
        for p in probes.iter().filter(|p| p.disappeared) {
            println!("  [退化] {} -> {:?}", p.input, p.expected);
            for step in &p.steps {
                println!(
                    "    {} => rank={:?}, top1={}",
                    step.prefix, step.rank, step.top1_text
                );
            }
        }
        return;
    }

    let summary = run_eval(&mut engine, &cases);
    let pct = |hits: usize| -> f64 {
        if summary.total == 0 {
            0.0
        } else {
            hits as f64 * 100.0 / summary.total as f64
        }
    };
    println!("评测用例总数: {}", summary.total);
    println!(
        "Top-1  命中: {}/{} ({:.1}%)",
        summary.top1_hits,
        summary.total,
        pct(summary.top1_hits)
    );
    println!(
        "Top-3  命中: {}/{} ({:.1}%)",
        summary.top3_hits,
        summary.total,
        pct(summary.top3_hits)
    );
    println!(
        "Top-10 命中: {}/{} ({:.1}%)",
        summary.top10_hits,
        summary.total,
        pct(summary.top10_hits)
    );
    println!(
        "平均耗时: {:.1} us | P99 耗时: {} us",
        summary.avg_us, summary.p99_us
    );
    if !summary.category_stats.is_empty() {
        println!("分场景命中统计:");
        for stat in &summary.category_stats {
            println!(
                "  {:<10} Top-1 {}/{} | Top-3 {}/{}",
                stat.category, stat.top1_hits, stat.total, stat.top3_hits, stat.total
            );
        }
    }

    if !summary.misses.is_empty() {
        println!("未进入 Top-3 的用例:");
        for miss in &summary.misses {
            println!(
                "  {} => 期望 {:?}, 实际排名 {:?}, Top1={}",
                miss.input, miss.expected, miss.best_rank, miss.top1_text
            );
        }
    }
}
