//! 算法引擎装配与配置/词库热重载模块

use std::fs;

use log::info;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::table::LexiconTable;
use waylan_algo::{AlgoConfig, AlgoEngine};
use waylan_core::config::AppConfig;

use super::loader::load_lexicon_table;

/// 统一加载英文与符号表情辅助词库管理器并挂载到算法引擎（冷启动与热重载共用）
fn attach_auxiliary_suggesters(engine: &mut AlgoEngine) {
    let aux_paths = AppConfig::aux_dict_search_paths();
    let (english_suggester, symbol_suggester) =
        waylan_algo::load_auxiliary_managers_from_paths(&aux_paths);
    engine.resources_mut().english_suggester = english_suggester;
    engine.resources_mut().symbol_suggester = symbol_suggester;
}

/// 基于指定配置与装配好的基础字词库构建完整的算法引擎
pub fn create_engine_with_table(table: LexiconTable, cfg: &AppConfig) -> AlgoEngine {
    let algo_config = AlgoConfig::from(cfg);
    let mut engine = AlgoEngine::new(table, algo_config);

    let phrase_path = CustomPhraseManager::default_path();
    engine.resources_mut().custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);
    let user_dict = AppConfig::user_dict_path();
    engine.set_user_dict_path(&user_dict);
    if user_dict.exists() {
        info!(
            "[字库引擎] 成功载入用户自适应个性化历史词库: {:?} (词条数: {})",
            user_dict,
            engine.user_adaptive_total_entries()
        );
    }

    attach_auxiliary_suggesters(&mut engine);

    if cfg.pinyin.traditional_chinese {
        info!(
            "[简繁词库] 全局简繁转换两级词库就绪 (单字映射: {} 个, 消歧短语: {} 组)",
            engine.resources().opencc_dict().chars.len(),
            engine.resources().opencc_dict().phrases_len()
        );
    }

    engine
}

/// 统一创建装配全量词库、自定义短语与用户个性化学习库的标准拼音算法引擎
pub fn create_standard_engine(cfg: &AppConfig) -> AlgoEngine {
    let table = load_lexicon_table();
    create_engine_with_table(table, cfg)
}

/// 对已有算法引擎执行热重载（重新载入配置、自定义短语、用户自学习库与辅助词典）
pub fn reload_engine_resources(engine: &mut AlgoEngine, cfg: &AppConfig) {
    engine.config = AlgoConfig::from(cfg);

    let phrase_path = CustomPhraseManager::default_path();
    engine.resources_mut().custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);

    let user_dict_path = AppConfig::user_dict_path();
    if user_dict_path.exists() {
        if let Ok(m) = fs::metadata(&user_dict_path) {
            if m.len() == 0 {
                info!("[热重载] 检测到用户词库文件为空，重置内存用户自适应模型");
                engine.reset_user_adaptive_model();
                engine.set_user_dict_path(&user_dict_path);
            } else if !engine.is_same_wal_file(&user_dict_path) {
                engine.set_user_dict_path(&user_dict_path);
            }
        }
    } else {
        info!("[热重载] 检测到用户词库文件已被删除或清空，重置内存用户自适应模型");
        engine.reset_user_adaptive_model();
        engine.set_user_dict_path(&user_dict_path);
    }

    attach_auxiliary_suggesters(engine);
}

/// 执行默认的算法引擎词库与自定义短语热重载（从磁盘重新加载最新 AppConfig 并应用）
pub fn apply_default_engine_reload(engine: &mut AlgoEngine) {
    let cfg = AppConfig::load_or_create();
    reload_engine_resources(engine, &cfg);
}

/// 执行默认的输入法运行时完整配置与词库热重载（从磁盘重新加载最新 AppConfig 并应用）
pub fn apply_default_runtime_reload(runtime: &mut crate::runtime::ImeRuntime) {
    let cfg = AppConfig::load_or_create();
    runtime.reload_config(&cfg);
}
