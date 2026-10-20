//! 统一算法引擎装配工厂与全量词库资产加载器
//!
//! # 设计原理
//! - **实现初衷**：为守护进程启动、图形化设置中心、测试示例及协议端提供单一真实的算法引擎装配入口，
//!   消除上层反复编写的词库加载、符号链接构建与资源拼接逻辑。
//! - **核心优势**：高内聚资源装配，统一规范多级词库查找顺序与自动回退兜底，100% 保障冷启动与热重载一致性。
//! - **代价与局限**：首次加载全量主词库需消耗数十毫秒级内存映射与解析时间。

use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use log::{info, warn};
pub use waylan_algo::custom_phrase::load_auxiliary_managers;
pub use waylan_algo::custom_phrase::{AuxiliaryDict, CustomPhraseManager};
use waylan_algo::opencc::OpenCcDict;
use waylan_algo::table::{CompactMmapLexicon, LexiconTable};
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::{AlgoConfig, AlgoEngine};
use waylan_core::config::AppConfig;

/// 自动建立用户数据目录 (~/.local/share/waylan/) 与外部全量词库的符号链接
pub fn ensure_user_dictionaries_linked() {
    let Some(home) = env::var("HOME").ok().filter(|s| !s.is_empty()) else {
        return;
    };
    let user_data_dir = PathBuf::from(home).join(".local/share/waylan");
    if fs::create_dir_all(&user_data_dir).is_err() {
        return;
    }

    let pairs = [
        ("base.dict.bin", AppConfig::base_dict_search_paths()),
        ("aux.dict.bin", AppConfig::aux_dict_search_paths()),
        ("opencc.dict.bin", AppConfig::opencc_dict_search_paths()),
    ];

    for (file_name, candidates) in pairs {
        let user_file = user_data_dir.join(file_name);
        if !user_file.exists() {
            for cand in candidates {
                if cand.is_file() && cand != user_file {
                    #[cfg(unix)]
                    let link_res = symlink(&cand, &user_file);
                    #[cfg(not(unix))]
                    let link_res = fs::copy(&cand, &user_file).map(|_| ());
                    if link_res.is_ok() {
                        info!(
                            "[字库引擎] 已自动关联词库至用户数据目录: {:?} -> {:?}",
                            user_file, cand
                        );
                        break;
                    }
                }
            }
        }
    }
}

/// 按照多级搜索策略加载汉语全量基础字词库
pub fn load_lexicon_table() -> LexiconTable {
    ensure_user_dictionaries_linked();
    info!("[字库引擎] 正在定位并加载字词库索引...");
    let search_paths = AppConfig::base_dict_search_paths();

    for path in &search_paths {
        if path.exists() {
            let load_start = Instant::now();
            match LexiconTable::load_binary(path) {
                Ok(loaded) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "  - 成功加载全量汉语词库: {:?} (耗时: {:.2} ms, 拼音键数: {} 组)",
                        path,
                        elapsed.as_secs_f64() * 1000.0,
                        loaded.len()
                    );
                    return loaded;
                }
                Err(e) => {
                    warn!("  - 读取词库文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    warn!("  - 未检测到有效的基础外部词库文件，回退到内置基础测试词库。");
    LexiconTable::with_sample_data()
}

/// 按照多级搜索策略加载辅助与混输词库（英文与符号表情）
pub fn load_auxiliary_dict() -> AuxiliaryDict {
    info!("[辅助词库] 正在定位并加载混输与符号词库索引...");
    let search_paths = AppConfig::aux_dict_search_paths();

    for path in &search_paths {
        if path.exists() {
            let load_start = Instant::now();
            match AuxiliaryDict::load_from_file(path) {
                Ok(dict) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "  - 成功加载辅助混输词库: {:?} (耗时: {:.2} ms, 英文词数: {}, 符号表情: {} 组)",
                        path,
                        elapsed.as_secs_f64() * 1000.0,
                        dict.english_words.len(),
                        dict.symbols.len()
                    );
                    return dict;
                }
                Err(e) => {
                    warn!("  - 读取辅助词库文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    info!("  - 未检测到外部辅助词库文件，使用系统内置保底英文与符号表情库。");
    AuxiliaryDict::with_default_data()
}

/// 按照多级搜索策略加载简繁转换两级词库
pub fn load_opencc_dict() -> Arc<OpenCcDict> {
    OpenCcDict::global()
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

    let (english_suggester, symbol_suggester) = load_auxiliary_managers();
    engine.resources_mut().english_suggester = english_suggester;
    engine.resources_mut().symbol_suggester = symbol_suggester;

    // 简繁转换词库按需懒加载，仅在启用繁体输出时于启动阶段触发载入并输出就绪日志
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

    // 1. 重新载入自定义短语
    let phrase_path = CustomPhraseManager::default_path();
    engine.resources_mut().custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);

    // 2. 重新检测并同步用户自适应词库
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

    // 3. 重新载入英文与符号表情辅助词库管理器
    let (english_suggester, symbol_suggester) = load_auxiliary_managers();
    engine.resources_mut().english_suggester = english_suggester;
    engine.resources_mut().symbol_suggester = symbol_suggester;
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

/// 词库概况与统计查询服务门面
///
/// # 设计原理
/// - **实现初衷**：为偏好设置中心（waylan-settings）与外部诊断工具提供统一的词库状态查询入口，
///   消除上层直接触碰底层 `CompactMmapLexicon` 或 `UserAdaptiveModel` 的穿透性依赖坏味道。
/// - **核心优势**：集中收敛系统词库与自造词模型统计口径，保障 API 边界清晰内聚。
pub struct LexiconSummaryService;

impl LexiconSummaryService {
    /// 查询系统预编译主词库（base.dict.bin）的状态描述信息
    pub fn query_system_dict_info() -> String {
        for path in AppConfig::base_dict_search_paths() {
            if path.exists() {
                if let Ok(lexicon) = CompactMmapLexicon::open(&path) {
                    return format!(
                        "系统预编译词库: base.dict.bin (约 {:.1} 万组词条已就绪)",
                        lexicon.num_candidates as f64 / 10_000.0
                    );
                }
            }
        }
        "系统预编译词库: base.dict.bin (未找到或未就绪)".to_string()
    }

    /// 查询用户自适应个性化历史词模型的记录总数
    pub fn query_user_adaptive_entry_count() -> usize {
        let user_dict_path = AppConfig::user_dict_path();
        UserAdaptiveModel::load_or_replay_wal(&user_dict_path)
            .map(|m| m.total_entries())
            .unwrap_or(0)
    }

    /// 加载并返回辅助混输词库（英文单词与特殊符号）
    pub fn load_auxiliary_dict() -> AuxiliaryDict {
        load_auxiliary_dict()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // 允许为 0 或正常数量，不应发生 panic
        let _ = user_count;

        let aux = LexiconSummaryService::load_auxiliary_dict();
        assert!(!aux.english_words.is_empty(), "内置英文单词应不为空");
        assert!(!aux.symbols.is_empty(), "内置符号表情应不为空");
    }
}
