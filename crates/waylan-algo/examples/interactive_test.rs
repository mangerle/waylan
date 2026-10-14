use std::io::{self, Write};
use std::time::Instant;
use waylan_algo::custom_phrase::AuxiliaryDict;
use waylan_algo::{table::LexiconTable, types::AlgoConfig, AlgoEngine};

fn main() {
    println!("============================================================");
    println!("       微澜 (WayLan) 输入法核心引擎 - 交互式打字测试终端    ");
    println!("============================================================");

    let dict_path = if std::path::Path::new("data/base.dict.bin").exists() {
        "data/base.dict.bin"
    } else if std::path::Path::new("/usr/share/waylan/data/base.dict.bin").exists() {
        "/usr/share/waylan/data/base.dict.bin"
    } else {
        "data/base.dict.bin"
    };

    println!(
        "[1/2] 正在载入全量汉语主词库 (172万词条): {} ...",
        dict_path
    );
    let start_load = Instant::now();
    let table = LexiconTable::load_binary(dict_path).expect("主词库加载失败");
    println!(
        "      全量主词库载入成功！耗时: {:.2} ms",
        start_load.elapsed().as_secs_f64() * 1000.0
    );
    if let Some(ref bg) = table.compact_bigram {
        println!(
            "      联动挂载高密度 CSR 统计语言模型: 收录 {} 万二元转移词对 (零拷贝 mmap)",
            bg.total_pairs / 10000
        );
    }

    let mut engine = AlgoEngine::new(table, AlgoConfig::default());

    let aux_path = if std::path::Path::new("data/aux.dict.bin").exists() {
        "data/aux.dict.bin"
    } else if std::path::Path::new("/usr/share/waylan/data/aux.dict.bin").exists() {
        "/usr/share/waylan/data/aux.dict.bin"
    } else {
        "data/aux.dict.bin"
    };
    if std::path::Path::new(aux_path).exists() {
        println!("[2/2] 正在载入辅助混输词库: {} ...", aux_path);
        if let Ok(aux_dict) = AuxiliaryDict::load_from_file(aux_path) {
            engine.load_auxiliary_dict(aux_dict);
            println!("      辅助混输库载入成功 (包含 2.3万 英文与 5000+ 符号表情)！");
        }
    }

    println!("------------------------------------------------------------");
    println!("测试指南：");
    println!("  1. 拼音打字测试：输入 women、xian、zhongguo、weilan");
    println!("  2. 中英混输测试：输入 rust、linux、readme、cargo");
    println!("  3. 符号表情测试：输入 yazi、dianzan、bianfu");
    println!("  4. 颜文字测试：  输入 vv");
    println!("  5. 动态扩展特性：输入 riqi、shijian、v1+2*3");
    println!("  6. 退出测试：    输入 q 或 exit");
    println!("============================================================\n");

    let stdin = io::stdin();
    let mut line = String::new();

    loop {
        print!("微澜输入 >> ");
        io::stdout().flush().unwrap();
        line.clear();
        if stdin.read_line(&mut line).is_err() || line.is_empty() {
            break;
        }

        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        if input == "q" || input == "exit" || input == "quit" {
            println!("退出测试，祝您使用愉快！");
            break;
        }

        // 清空引擎预编辑
        engine.reset();

        // 模拟逐字敲键，测量真实响应延迟（分别记录末键延迟、平均每键延迟与全句累积耗时）
        let start = Instant::now();
        let mut last_char_us = 0;
        for ch in input.chars() {
            let t0 = Instant::now();
            engine.push_char(ch);
            last_char_us = t0.elapsed().as_micros();
        }
        let total_us = start.elapsed().as_micros();
        let avg_us = total_us / input.len().max(1) as u128;
        let candidates = engine.candidates();

        let preedit = engine.formatted_preedit();
        println!(
            "  切分音节: [{}]  (末键延迟: {} µs, 均键延迟: {} µs, {}键累积: {} µs)",
            preedit,
            last_char_us,
            avg_us,
            input.len(),
            total_us
        );
        if candidates.is_empty() {
            println!("  [无匹配候选词]");
        } else {
            print!("  候选词列表: ");
            for (idx, cand) in candidates.iter().take(8).enumerate() {
                let comment_str = if let Some(ref c) = cand.comment {
                    format!("({})", c)
                } else {
                    String::new()
                };
                print!("{}.{}{}\t", idx + 1, cand.text, comment_str);
            }
            println!();
        }
        println!();
    }
}
