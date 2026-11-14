use std::time::Instant;
use waylan_algo::builder::{DictBuildOptions, DictBuilder};

fn main() {
    println!("=== 正在编译汉语精简主词库 (base.dict.bin) ===");
    let start = Instant::now();

    let input_paths = [
        "data/dicts/1_chars/rime_mint.chars.dict.yaml",
        "data/dicts/2_base/rime_mint.base.dict.yaml",
        "data/dicts/3_ext/rime_mint.ext.dict.yaml",
        "data/dicts/3_ext/rime_mint.places.dict.yaml",
    ];

    let output_path = "data/base.dict.bin";

    let stats = DictBuilder::build_compact_binary_dict_with_options(
        &input_paths,
        output_path,
        DictBuildOptions::production(),
    )
    .expect("编译汉语紧凑主词库失败");

    let elapsed = start.elapsed().as_millis();
    let file_size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);

    println!("============================================================");
    println!("汉语主词库编译完成！总耗时: {} ms", elapsed);
    println!("- 扫描总行数:       {} 行", stats.total_lines);
    println!("- 提取有效词条数:   {} 条", stats.total_words);
    println!("- 唯一拼音键总数:   {} 个", stats.total_keys);
    println!("- 输出文件:         {}", output_path);
    println!(
        "- 生成二进制大小:   {} 字节 ({:.2} MB)",
        file_size,
        file_size as f64 / (1024.0 * 1024.0)
    );
    println!("============================================================");
}
