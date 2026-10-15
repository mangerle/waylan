use waylan_algo::custom_phrase::{
    AuxiliaryDict, CustomPhraseManager, EnglishSuggestionManager, SymbolSuggestionManager,
};

#[test]
fn test_custom_phrase_parse_and_lookup() {
    let mut manager = CustomPhraseManager::new();
    manager.parse_content("mymail=test@test.com\naddr 北京市海淀区");
    let mail_res = manager.lookup("mymail");
    assert_eq!(mail_res.len(), 1);
    assert_eq!(mail_res[0].text, "test@test.com");

    let addr_res = manager.lookup("addr");
    assert_eq!(addr_res.len(), 1);
    assert_eq!(addr_res[0].text, "北京市海淀区");
}

#[test]
fn test_custom_phrase_all_entries_remove_save() {
    let mut manager = CustomPhraseManager::new();
    manager.add_entry("mymail".to_string(), "a@b.com".to_string());
    manager.add_entry("addr".to_string(), "北京".to_string());

    let entries = manager.all_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], ("addr".to_string(), "北京".to_string()));

    assert!(manager.remove_entry("addr", "北京"));
    assert_eq!(manager.all_entries().len(), 1);
    assert!(!manager.remove_entry("non_exist", "none"));
}

#[test]
fn test_english_suggestions() {
    let eng = EnglishSuggestionManager::new();
    let exact = eng.lookup("rust");
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].text, "rust");

    let prefix = eng.lookup("way");
    assert!(!prefix.is_empty());
    assert!(prefix
        .iter()
        .any(|c| c.text == "wayland" || c.text == "waylan"));
}

#[test]
fn test_symbol_suggestions() {
    let sym = SymbolSuggestionManager::new();
    let res = sym.lookup("dianzan");
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].text, "\u{1F44D}");
}

#[test]
fn test_auxiliary_dict_save_and_load() {
    let dict = AuxiliaryDict::with_default_data();
    assert!(!dict.english_words.is_empty());
    assert!(!dict.symbols.is_empty());

    let temp_path = std::env::temp_dir().join("test_aux_dict.bin");
    dict.save_to_file(&temp_path).expect("save aux dict");
    let loaded = AuxiliaryDict::load_from_file(&temp_path).expect("load aux dict");
    assert_eq!(dict, loaded);
    let _ = std::fs::remove_file(temp_path);
}

#[test]
fn test_custom_phrase_newline_escape_roundtrip() {
    let mut manager = CustomPhraseManager::new();
    manager.parse_content("sign=第一行\\n第二行\\t第三行\nmultiline 多行内容\\n第二行");

    let sign_cands = manager.lookup("sign");
    assert_eq!(sign_cands.len(), 1);
    assert_eq!(sign_cands[0].text, "第一行\n第二行\t第三行");

    let multi_cands = manager.lookup("multiline");
    assert_eq!(multi_cands.len(), 1);
    assert_eq!(multi_cands[0].text, "多行内容\n第二行");

    let temp_path = std::env::temp_dir().join(format!("test_phrase_{}.txt", std::process::id()));
    manager.save_to_file(&temp_path).expect("save phrase file");

    let mut reloaded = CustomPhraseManager::new();
    let content = std::fs::read_to_string(&temp_path).expect("read phrase file");
    reloaded.parse_content(&content);

    let reloaded_sign = reloaded.lookup("sign");
    assert_eq!(reloaded_sign.len(), 1);
    assert_eq!(reloaded_sign[0].text, "第一行\n第二行\t第三行");

    let _ = std::fs::remove_file(temp_path);
}

#[test]
fn test_custom_phrase_dynamic_macro_expansion() {
    let mut manager = CustomPhraseManager::new();
    manager.parse_content(
        "dt=#date\n\
         cn_dt=#date(YYYY年MM月DD日)\n\
         tm=#time(HH:mm)\n\
         sig=签名：\\n#date\\n#time",
    );

    let now = chrono::Local::now();

    let dt_cands = manager.lookup("dt");
    assert_eq!(dt_cands.len(), 1);
    assert_eq!(dt_cands[0].text, now.format("%Y-%m-%d").to_string());

    let cn_cands = manager.lookup("cn_dt");
    assert_eq!(cn_cands.len(), 1);
    assert_eq!(cn_cands[0].text, now.format("%Y年%m月%d日").to_string());

    let tm_cands = manager.lookup("tm");
    assert_eq!(tm_cands.len(), 1);
    assert_eq!(tm_cands[0].text, now.format("%H:%M").to_string());

    let sig_cands = manager.lookup("sig");
    assert_eq!(sig_cands.len(), 1);
    assert!(sig_cands[0].text.starts_with("签名：\n"));
    assert!(sig_cands[0]
        .text
        .contains(&now.format("%Y-%m-%d").to_string()));
}
