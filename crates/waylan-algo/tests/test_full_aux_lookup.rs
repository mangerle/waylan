use std::path::Path;
use waylan_algo::custom_phrase::{
    AuxiliaryDict, EnglishSuggestionManager, SymbolSuggestionManager,
};

#[test]
fn test_full_aux_english_and_symbols() {
    let path = if Path::new("../../data/aux.dict.bin").exists() {
        "../../data/aux.dict.bin"
    } else {
        "data/aux.dict.bin"
    };
    let dict = AuxiliaryDict::load_from_file(path).expect("加载 aux.dict.bin");
    assert!(dict.english_words.len() > 20000);
    assert!(dict.symbols.len() > 5000);

    let eng = EnglishSuggestionManager::from_words(dict.english_words);
    let sym = SymbolSuggestionManager::from_symbols(dict.symbols);

    // 1. 验证常用与专业开发英文
    let rust_cands = eng.lookup("rust");
    assert!(!rust_cands.is_empty());
    assert_eq!(rust_cands[0].text, "rust");

    let readme_cands = eng.lookup("readme");
    assert!(!readme_cands.is_empty());
    assert!(readme_cands
        .iter()
        .any(|c| c.text.to_lowercase().contains("readme")));

    // 2. 验证特殊符号表情 (鸭子: yazi)
    let yazi_cands = sym.lookup("yazi");
    assert!(!yazi_cands.is_empty());
    assert!(yazi_cands.iter().any(|c| c.text == "\u{1F986}"));

    // 3. 验证颜文字 (vv -> 颜文字表情)
    let vv_cands = sym.lookup("vv");
    assert!(!vv_cands.is_empty());
    assert_eq!(vv_cands[0].comment.as_deref(), Some("颜文字"));
}
