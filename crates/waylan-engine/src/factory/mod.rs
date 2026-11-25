//! 统一算法引擎装配工厂、词库资产加载与统计诊断门面
//!
//! # 设计原理
//! - **实现初衷**：将原本混杂在单一 `factory.rs` 中的多级词库符号链接/加载、
//!   引擎装配/热重载与设置中心统计门面按职责拆分为三个高内聚子模块。
//! - **核心优势**：职责清晰、零重复资产装配逻辑，并对统计查询增加文件签名缓存避免重复 WAL 重放。
//! - **代价与局限**：首次冷加载全量主词库仍需数十毫秒级内存映射时间。

mod builder;
mod loader;
mod summary;

pub use builder::{
    apply_default_engine_reload, apply_default_runtime_reload, create_engine_with_table,
    create_standard_engine, reload_engine_resources,
};
pub use loader::{
    ensure_user_dictionaries_linked, load_auxiliary_dict, load_lexicon_table, load_opencc_dict,
};
pub use summary::LexiconSummaryService;
pub use waylan_algo::custom_phrase::load_auxiliary_managers;
pub use waylan_algo::custom_phrase::{AuxiliaryDict, CustomPhraseManager};

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_algo::table::LexiconTable;
    use waylan_algo::{AlgoConfig, AlgoEngine};
    use waylan_core::config::AppConfig;

    #[test]
    fn test_create_engine_with_sample_table() {
        let table = LexiconTable::with_sample_data();
        let cfg = AppConfig::default();
        let engine = create_engine_with_table(table, &cfg);
        assert!(!engine.table().is_empty());
    }

    #[test]
    fn test_reload_engine_resources() {
        let table = LexiconTable::with_sample_data();
        let mut cfg = AppConfig::default();
        cfg.ui.page_size = 7;
        let mut engine = create_engine_with_table(table, &cfg);

        cfg.ui.page_size = 9;
        reload_engine_resources(&mut engine, &cfg);
        assert_eq!(engine.config.page_size, 9);
    }

    #[test]
    fn test_apply_default_engine_and_runtime_reload() {
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        apply_default_engine_reload(&mut engine);
        assert_eq!(engine.raw_input(), "");

        let mut runtime = crate::runtime::ImeRuntime::with_engine(AlgoEngine::with_sample_data(
            AlgoConfig::default(),
        ));
        apply_default_runtime_reload(&mut runtime);
        assert_eq!(runtime.core.engine.raw_input(), "");
    }

    #[test]
    fn test_lexicon_summary_service() {
        let sys_info = LexiconSummaryService::query_system_dict_info();
        assert!(!sys_info.is_empty(), "系统词库信息不应为空");

        let user_count = LexiconSummaryService::query_user_adaptive_entry_count();
        let _ = user_count;

        let aux = LexiconSummaryService::load_auxiliary_dict();
        assert!(!aux.english_words.is_empty(), "内置英文单词应不为空");
        assert!(!aux.symbols.is_empty(), "内置符号表情应不为空");
    }
}
