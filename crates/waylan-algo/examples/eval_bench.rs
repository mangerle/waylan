//! 微澜离线量化评测与逐键探针命令行工具

use std::env;
use std::fs;
use std::path::Path;
use std::time::Instant;

use waylan_algo::eval_bench::{
    builtin_regression_cases, parse_eval_tsv, run_eval, run_probe, EvalCase,
};
use waylan_algo::{AlgoConfig, AlgoEngine, CompactMmapBigram, LexiconTable};

fn load_cases_from_args(args: &[String]) -> Vec<EvalCase> {
    if let Some(pos) = args.iter().position(|a| a == "--cases") {
        if let Some(path) = args.get(pos + 1) {
            if let Ok(content) = fs::read_to_string(path) {
                return parse_eval_tsv(&content);
            }
        }
    }
    builtin_regression_cases()
}

fn load_engine() -> Result<AlgoEngine, String> {
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
    Ok(AlgoEngine::new(table, AlgoConfig::default()))
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let probe_mode = args.iter().any(|a| a == "--probe");
    let cases = load_cases_from_args(&args);

    let mut engine = match load_engine() {
        Ok(e) => e,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };

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
