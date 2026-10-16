//! 用户自定义短语与混输辅助词库模块
//!
//! 整合用户文本短语映射（`phrases.txt`）与紧凑二进制辅助混输词库（`aux.dict.bin`）。

mod aux_dict;
pub mod aux_manager;
mod data;
mod dynamic_macro;
mod manager;

pub use aux_dict::{AuxiliaryDict, AUX_DICT_MAGIC};
pub use aux_manager::{EnglishSuggestionManager, SymbolSuggestionManager};
pub use manager::CustomPhraseManager;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_phrase_manager_parse_and_lookup() {
        let content = r#"
# 测试短语表
mymail=user@example.com
myphone 13800138000
multi=line1\nline2\ttab
"#;
        let mut manager = CustomPhraseManager::new();
        manager.parse_content(content);

        let email_res = manager.lookup("mymail");
        assert_eq!(email_res.len(), 1);
        assert_eq!(email_res[0].text, "user@example.com");

        let phone_res = manager.lookup("MYPHONE");
        assert_eq!(phone_res.len(), 1);
        assert_eq!(phone_res[0].text, "13800138000");

        let multi_res = manager.lookup("multi");
        assert_eq!(multi_res.len(), 1);
        assert_eq!(multi_res[0].text, "line1\nline2\ttab");

        assert!(manager.remove_entry("mymail", "user@example.com"));
        assert!(manager.lookup("mymail").is_empty());
    }

    #[test]
    fn test_auxiliary_dict_serialization_roundtrip() {
        let dict = AuxiliaryDict {
            english_words: vec![
                ("rust".to_string(), 100_000),
                ("waylan".to_string(), 90_000),
            ],
            symbols: vec![
                ("wx".to_string(), "微笑".to_string(), "表情".to_string()),
                ("jt".to_string(), "->".to_string(), "箭头".to_string()),
            ],
        };

        let bin_path = std::env::temp_dir().join(format!(
            "test_aux_dict_roundtrip_{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&bin_path);

        dict.save_to_file(&bin_path).expect("序列化保存应成功");
        assert!(bin_path.exists());

        let loaded = AuxiliaryDict::load_from_file(&bin_path).expect("加载反序列化应成功");
        assert_eq!(loaded.english_words, dict.english_words);
        assert_eq!(loaded.symbols, dict.symbols);
        let _ = std::fs::remove_file(&bin_path);
    }
}
