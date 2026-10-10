use std::fs;
use waylan_algo::opencc::OpenCcDict;
use waylan_algo::types::AlgoCandidate;

#[test]
fn test_fmm_disambiguation_phrases_and_chars() {
    let dict = OpenCcDict::global();

    // 一简对多繁词组消歧验证
    assert_eq!(dict.convert_text("头发"), "頭髮");
    assert_eq!(dict.convert_text("理发"), "理髮");
    assert_eq!(dict.convert_text("发展"), "發展");
    assert_eq!(dict.convert_text("皇后"), "皇后");
    assert_eq!(dict.convert_text("以后"), "以後");
    assert_eq!(dict.convert_text("面条"), "麪條");
    assert_eq!(dict.convert_text("牛肉面"), "牛肉麪");
    assert_eq!(dict.convert_text("手表"), "手錶");
    assert_eq!(dict.convert_text("干净"), "乾淨");
    assert_eq!(dict.convert_text("干部"), "幹部");
}

#[test]
fn test_single_char_multi_traditional_expansion() {
    let dict = OpenCcDict::global();
    let mut cands = vec![
        AlgoCandidate::new("头发", "tou'fa", 1000),
        AlgoCandidate::new("发", "fa", 800),
        AlgoCandidate::new("后", "hou", 700),
    ];

    dict.convert_candidates(&mut cands);
    let texts: Vec<&str> = cands.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts[0], "頭髮");
    assert!(texts.contains(&"發"), "单字候选 发 应包含首选繁体 發");
    assert!(texts.contains(&"髮"), "单字候选 发 应展开备选繁体 髮");
    assert!(texts.contains(&"後"), "单字候选 后 应包含首选繁体 後");
    assert!(texts.contains(&"后"), "单字候选 后 应展开备选繁体 后");
}

#[test]
fn test_opencc_binary_save_and_load_roundtrip() {
    let dict = OpenCcDict::with_builtin_fallback();
    let temp_path = std::env::temp_dir().join("waylan_test_opencc.dict.bin");
    dict.save_to_file(&temp_path)
        .expect("保存简繁二进制词库应成功");

    let loaded = OpenCcDict::load_from_file(&temp_path).expect("加载简繁二进制词库应成功");
    assert_eq!(loaded.convert_text("头发和牛肉面"), "頭髮和牛肉麪");
    assert_eq!(loaded.convert_text("皇后的手表"), "皇后的手錶");

    let _ = fs::remove_file(&temp_path);
}
