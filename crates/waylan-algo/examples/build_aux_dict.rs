use std::time::Instant;
use waylan_algo::builder::DictBuilder;

fn main() {
    println!("=== 正在编译全量辅助与混输二进制词库 (aux.dict.bin) ===");
    let start = Instant::now();

    let en_paths = [
        "data/dicts/4_english/rime_ice.en.dict.yaml",
        "data/dicts/4_english/rime_ice.en_ext.dict.yaml",
    ];

    let sym_paths = [
        "data/dicts/5_emoji/other_emoji.dict.yaml",
        "data/dicts/6_kaomoji/other_kaomoji.dict.yaml",
    ];

    let output_path = "data/aux.dict.bin";

    let dict = DictBuilder::build_auxiliary_dict(&en_paths, &sym_paths, output_path)
        .expect("编译辅助词库失败");

    let elapsed = start.elapsed().as_millis();
    let file_size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);

    println!("============================================================");
    println!("编译完成！耗时: {} ms", elapsed);
    println!("- 英文词库收录数:   {} 条", dict.english_words.len());
    println!("- 符号/Emoji/颜文字: {} 条", dict.symbols.len());
    println!("- 输出文件:         {}", output_path);
    println!("- 生成二进制文件大小: {} 字节 ({:.2} KB)", file_size, file_size as f64 / 1024.0);
    println!("============================================================");
}
