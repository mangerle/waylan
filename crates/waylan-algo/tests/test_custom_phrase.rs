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
