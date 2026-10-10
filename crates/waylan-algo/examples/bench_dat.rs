use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::Instant;
use waylan_algo::dat::{DatReader, DoubleArrayTrie};

fn main() {
    println!("=== 测试 148 万真实拼音键 DAT + Tail 压缩构建性能 ===");
    let t0 = Instant::now();
    let mut keys = Vec::with_capacity(2_000_000);

    let files = [
        "data/dicts/1_chars/rime_mint.chars.dict.yaml",
        "data/dicts/2_base/rime_mint.base.dict.yaml",
        "data/dicts/3_ext/rime_mint.ext.dict.yaml",
        "data/dicts/3_ext/rime_mint.places.dict.yaml",
    ];

    for path in &files {
        let Ok(f) = File::open(path) else {
            eprintln!("无法打开 {}", path);
            return;
        };
        let reader = BufReader::new(f);
        for line in reader.lines() {
            let Ok(l) = line else { continue };
            let trimmed = l.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("---") {
                continue;
            }
            let parts: Vec<&str> = trimmed.split('\t').collect();
            if parts.len() >= 2 {
                let p = waylan_algo::builder::normalize_pinyin_tones(parts[1].trim());
                if !p.is_empty() && p.bytes().all(|b: u8| b == b'\'' || b.is_ascii_lowercase()) {
                    keys.push(p);
                }
            }
        }
    }

    println!("读取完成，原始合法拼音条数: {} 条", keys.len());
    keys.sort();
    keys.dedup();
    println!(
        "去重后唯一拼音键: {} 个，耗时: {:?}",
        keys.len(),
        t0.elapsed()
    );

    let values: Vec<u32> = (0..keys.len() as u32).collect();
    let t_dat = Instant::now();
    println!("正在构建 DoubleArrayTrie (带 Tail 压缩)...");
    let dat = DoubleArrayTrie::build(&keys, &values);
    let dur = t_dat.elapsed();
    let base_check_mb = (dat.base.len() * 8) as f64 / 1024.0 / 1024.0;
    let tail_mb = dat.tail.len() as f64 / 1024.0 / 1024.0;
    println!("DAT 构建完成！耗时: {:?} | base/check 节点数: {} ({:.2} MB) | Tail 池: {:.2} MB | DAT 索引总计: {:.2} MB", 
        dur, dat.base.len(), base_check_mb, tail_mb, base_check_mb + tail_mb);

    let reader = DatReader::new(&dat.base, &dat.check, &dat.tail);
    // 抽样验证 2000 个键
    let step = keys.len() / 2000;
    for i in 0..2000 {
        let sample_idx = (i * step).min(keys.len() - 1);
        let k = &keys[sample_idx];
        let res = reader.exact_match(k);
        assert_eq!(res, Some(sample_idx as u32), "键 {} 匹配不一致", k);
    }
    println!("抽样验证 2000 个键 100% 正确！");
}
