use std::path::Path;
use std::time::Instant;
use waylan_algo::types::AlgoConfig;
use waylan_algo::table::LexiconTable;
use waylan_algo::AlgoEngine;

#[test]
fn test_bigram_mmap_end_to_end_typing() {
    let dict_path = Path::new("../../data/base.dict.bin");
    let fallback_path = Path::new("data/base.dict.bin");
    let real_dict_path = if dict_path.exists() {
        dict_path
    } else if fallback_path.exists() {
        fallback_path
    } else {
        println!("未检测到底层二进制词库，跳过大语料端到端实测");
        return;
    };

    println!("========================================================");
    println!("  微澜输入法 (WayLan) 高密度 CSR Bigram 端到端整句打字实测");
    println!("========================================================");

    // 1. 零拷贝加载主词典、全局词表与高密度二元转移概率矩阵
    let start_load = Instant::now();
    let table = LexiconTable::load_binary(real_dict_path).expect("加载主词典及Bigram转移矩阵失败");
    let load_duration = start_load.elapsed();

    println!("词典与转移矩阵载入耗时: {:?}", load_duration);
    assert!(table.compact_bigram.is_some(), "必须自动挂载同级 bigram.table.bin");
    let compact = table.compact_mmap.as_ref().expect("必须成功挂载一体化底库");

    let bigram = table.compact_bigram.as_ref().unwrap();
    println!("高密度 CSR Bigram 规格: 词表总数 = {}, 有效二元词对数 = {}", 
        bigram.vocab_size, bigram.total_pairs);
    assert!(bigram.total_pairs > 3_000_000, "正式大语料模型收录词对数应超过 300 万");
    assert_eq!(bigram.vocab_size, compact.vocab_size, "CSR 模型词表容量应与一体化底库词表容量保持一致");

    // 2. 验证底层转移亲和度检索
    let test_pairs = vec![
        ("北京", "时间"),
        ("人工", "智能"),
        ("操作", "系统"),
        ("自然", "语言"),
        ("语言", "处理"),
        ("清风", "拂面"),
        ("我们", "的"),
        ("世界", "各地"),
    ];

    println!("\n--- [阶段一] 典型二元转移亲和度检索验证 ---");
    for (w1, w2) in test_pairs {
        let affinity = table.get_bigram_affinity(w1, w2);
        if let Some(score) = affinity {
            let id1 = compact.get_id(w1).unwrap_or(0);
            let id2 = compact.get_id(w2).unwrap_or(0);
            println!("命中搭配: [{}] (ID:{}) -> [{}] (ID:{}), 亲和度量化得分 = {}", w1, id1, w2, id2, score);
        } else {
            println!("未命中搭配: [{}] -> [{}] (回退规则平滑)", w1, w2);
        }
    }

    // 3. 构建微澜端到端算法引擎
    let mut engine = AlgoEngine::new(table, AlgoConfig::default());

    // 4. 整句打字端到端测试用例集
    let test_sentences = vec![
        ("beijingshijian", "北京时间"),
        ("rengongzhineng", "人工智能"),
        ("caozuoxitong", "操作系统"),
        ("ziranyuyanchuli", "自然语言处理"),
        ("women", "我们"),
        ("qingfengfumian", "清风拂面"),
        ("jintianxingqitian", "今天星期天"),
        ("xi'an", "西安"),
        ("fangan", "方案"),
        ("nihso", "你好"),
    ];

    println!("\n--- [阶段二] 端到端整句击键打字解码实测 ---");
    for (input_pinyin, expected_prefix) in test_sentences {
        engine.reset();
        let start_type = Instant::now();
        
        // 模拟逐字敲击按键
        for ch in input_pinyin.chars() {
            engine.push_char(ch);
        }
        let type_duration = start_type.elapsed();

        let cands = engine.candidates();
        assert!(!cands.is_empty(), "输入拼音 [{}] 应产出有效候选", input_pinyin);

        let top1 = &cands[0];
        let preedit = engine.formatted_preedit();
        
        println!("输入按键: {:<18} | 智能分词: {:<18} | 首选词: {:<12} | 耗时: {:?}", 
            input_pinyin, preedit, top1.text, type_duration);

        // 显示前 3 位候选供审查
        let top3_desc: Vec<String> = cands.iter().take(3).map(|c| format!("{}({})", c.text, c.score)).collect();
        println!("   Top-3 候选列表: {}", top3_desc.join("  |  "));

        // 验证候选词列表中是否命中目标预期（正常词首选命中，容错词前 3 位备选命中）
        let is_hit = if input_pinyin == "nihso" {
            cands.iter().take(3).any(|c| c.text.starts_with(expected_prefix))
        } else {
            top1.text.starts_with(expected_prefix) || top1.text == expected_prefix
        };
        assert!(
            is_hit,
            "输入拼音 [{}] 候选列表应包含 [{}], 实际首选为 [{}]",
            input_pinyin, expected_prefix, top1.text
        );
    }

    println!("\n========================================================");
    println!("  微澜输入法高密度 CSR Bigram 端到端实测 100% 验证通过！");
    println!("========================================================");
}
