use waylan_algo::prism::PrismEngine;
use waylan_algo::table::LexiconTable;
use waylan_algo::types::AlgoConfig;
use waylan_algo::viterbi::ViterbiDecoder;
use waylan_algo::AlgoEngine;

#[test]
fn test_shux_prefix_expansion_end_to_end() {
    let dict_path = if std::path::Path::new("data/base.dict.bin").exists() {
        "data/base.dict.bin"
    } else if std::path::Path::new("/usr/share/waylan/data/base.dict.bin").exists() {
        "/usr/share/waylan/data/base.dict.bin"
    } else {
        "data/base.dict.bin"
    };

    if !std::path::Path::new(dict_path).exists() {
        return;
    }

    let table = LexiconTable::load_binary(dict_path).expect("主词库加载失败");
    let mut engine = AlgoEngine::new(table, AlgoConfig::default());

    // 1. 测试 shux 展开召回 熟悉、数学、属性等
    for ch in "shux".chars() {
        engine.push_char(ch);
    }

    let candidates = engine.candidates();
    assert!(!candidates.is_empty(), "shux 应能匹配出前缀补全候选词");

    let texts: Vec<&str> = candidates.iter().map(|c| c.text.as_str()).collect();
    assert!(texts.contains(&"熟悉") || texts.contains(&"数学"), "shux 应包含熟悉或数学");

    // 2. 测试 zhongg 展开召回 中国
    engine.reset();
    for ch in "zhongg".chars() {
        engine.push_char(ch);
    }
    let candidates_zg = engine.candidates();
    assert!(!candidates_zg.is_empty(), "zhongg 应能匹配出候选词");
    assert_eq!(candidates_zg[0].text, "中国", "zhongg 首选应为中国");
}

#[test]
fn test_timing_measurement() {
    let dict_path = if std::path::Path::new("data/base.dict.bin").exists() {
        "data/base.dict.bin"
    } else if std::path::Path::new("/usr/share/waylan/data/base.dict.bin").exists() {
        "/usr/share/waylan/data/base.dict.bin"
    } else {
        "data/base.dict.bin"
    };

    if !std::path::Path::new(dict_path).exists() {
        return;
    }

    let table = LexiconTable::load_binary(dict_path).expect("主词库加载失败");
    let mut engine = AlgoEngine::new(table, AlgoConfig::default());

    let inputs = ["dasdadasdasdasda", "wodeshijieshizhongghemieguo"];

    for input in &inputs {
        println!("\n=== 输入: {} (长度: {}) ===", input, input.len());
        engine.reset();
        let mut total_us = 0;
        let mut last_char_us = 0;
        for (idx, ch) in input.chars().enumerate() {
            let t0 = std::time::Instant::now();
            engine.push_char(ch);
            let elapsed = t0.elapsed().as_micros();
            total_us += elapsed;
            last_char_us = elapsed;
            println!("  [第 {:02} 键 '{}'] 耗时: {:5} µs", idx + 1, ch, elapsed);
        }
        let avg_us = total_us / input.len() as u128;
        println!("  >> 逐键累积总耗时: {} µs (约 {:.2} ms)", total_us, total_us as f64 / 1000.0);
        println!("  >> 平均每键延迟:   {} µs (约 {:.2} ms)", avg_us, avg_us as f64 / 1000.0);
        println!("  >> 最终一击耗时:   {} µs (约 {:.2} ms)", last_char_us, last_char_us as f64 / 1000.0);

        // 阶段耗时细分测试
        let t_p = std::time::Instant::now();
        let paths = PrismEngine::build_paths(input, &AlgoConfig::default().fuzzy, None);
        let p_dur = t_p.elapsed().as_micros();
        println!("  [Prism 耗时]: {} µs (切分出 {} 条路径)", p_dur, paths.len());

        let t_v = std::time::Instant::now();
        for p in paths.iter().take(2) {
            let _ = ViterbiDecoder::decode_sentence(&p.syllables, &p.remaining, engine.table());
        }
        let v_dur = t_v.elapsed().as_micros();
        println!("  [Viterbi 耗时]: {} µs", v_dur);

        if let Some(best) = paths.first() {
            let t_ext = std::time::Instant::now();
            let _ = engine.table().lookup_prefix_extension(&best.syllables, &best.remaining);
            let ext_dur = t_ext.elapsed().as_micros();
            println!("  [PrefixExt 耗时]: {} µs", ext_dur);
        }
    }
}


