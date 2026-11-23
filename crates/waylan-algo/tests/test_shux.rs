use waylan_algo::AlgoEngine;
use waylan_algo::decoder::{
    BeamDecodeOptions, LatticeOptions, PositionalBeamDecoder, SyllableLattice,
};
use waylan_algo::table::LexiconTable;
use waylan_algo::types::AlgoConfig;

#[test]
fn test_shux_prefix_expansion_end_to_end() {
    let dict_path = if std::path::Path::new("/home/wsl/waylan/data/base.dict.bin").exists() {
        "/home/wsl/waylan/data/base.dict.bin"
    } else if std::path::Path::new("../../data/base.dict.bin").exists() {
        "../../data/base.dict.bin"
    } else if std::path::Path::new("data/base.dict.bin").exists() {
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
    assert!(
        texts.contains(&"熟悉") || texts.contains(&"数学"),
        "shux 应包含熟悉或数学"
    );

    // 2. 测试 zhongg 展开召回 中国
    engine.reset();
    for ch in "zhongg".chars() {
        engine.push_char(ch);
    }
    let candidates_zg = engine.candidates();
    assert!(!candidates_zg.is_empty(), "zhongg 应能匹配出候选词");
    assert_eq!(candidates_zg[0].text, "中国", "zhongg 首选应为中国");

    // 3. 测试 chongg：预编辑文本应忠实呈现按键切分（chong'g），不自动脑补展开为 chong'gou，但候选仍应能召回重构
    engine.reset();
    for ch in "chongg".chars() {
        engine.push_char(ch);
    }
    assert_eq!(
        engine.formatted_preedit(),
        "chong'g",
        "chongg 预编辑文本应为 chong'g，不应自动补全为 chong'gou"
    );
    let candidates_cg = engine.candidates();
    assert!(
        candidates_cg.iter().any(|c| c.text == "重构"),
        "chongg 候选词列表仍应包含前缀预测候选词「重构」"
    );
}

#[test]
fn test_timing_measurement() {
    let dict_path = if std::path::Path::new("/home/wsl/waylan/data/base.dict.bin").exists() {
        "/home/wsl/waylan/data/base.dict.bin"
    } else if std::path::Path::new("../../data/base.dict.bin").exists() {
        "../../data/base.dict.bin"
    } else if std::path::Path::new("data/base.dict.bin").exists() {
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
        println!(
            "  >> 逐键累积总耗时: {} µs (约 {:.2} ms)",
            total_us,
            total_us as f64 / 1000.0
        );
        println!(
            "  >> 平均每键延迟:   {} µs (约 {:.2} ms)",
            avg_us,
            avg_us as f64 / 1000.0
        );
        println!(
            "  >> 最终一击耗时:   {} µs (约 {:.2} ms)",
            last_char_us,
            last_char_us as f64 / 1000.0
        );

        // 阶段耗时细分测试
        let t_l = std::time::Instant::now();
        let lattice = SyllableLattice::build(input, &LatticeOptions::default());
        let l_dur = t_l.elapsed().as_micros();
        println!("  [Lattice 耗时]: {} µs", l_dur);

        let t_b = std::time::Instant::now();
        let _ = PositionalBeamDecoder::decode(
            &lattice,
            engine.table(),
            None,
            &BeamDecodeOptions::default(),
        );
        let b_dur = t_b.elapsed().as_micros();
        println!("  [BeamV2 耗时]: {} µs", b_dur);
    }
}
