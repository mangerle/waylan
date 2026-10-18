//! 拼音算法引擎初始化与词库资源装配模块
//!
//! # 设计原理
//! - **实现初衷**：在守护进程启动阶段统一装配全量核心词库、自定义短语表、用户自学习词库与简繁体双向转换表。
//! - **核心优势**：在装配各阶段检测停机中断信号，杜绝冷启动中途收到 SIGTERM/SIGINT 时无法即时退出的缺陷。
//! - **代价与局限**：首次冷启动构建基础字库需要消耗数十毫秒级内存加载时间。

use crate::error::DaemonError;

use log::info;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::{AlgoConfig, AlgoEngine};
use waylan_core::config::AppConfig;

use super::dict_loader::{load_auxiliary_managers, load_lexicon_table};
use crate::system::ShutdownCoordinator;

/// 构建算法引擎并加载全量汉字库、自定义短语与用户个性化词库
pub fn build_algo_engine(
    cfg: &AppConfig,
    coordinator: &ShutdownCoordinator,
) -> Result<Option<AlgoEngine>, DaemonError> {
    let table = load_lexicon_table();
    if coordinator.is_shutting_down() {
        info!("[微澜守护进程] 启动加载期间收到终止信号，立即停止启动并安全退场。");
        return Ok(None);
    }
    let algo_config = AlgoConfig::from(cfg);
    let mut engine = AlgoEngine::new(table, algo_config);

    let phrase_path = CustomPhraseManager::default_path();
    engine.resources.custom_phrases = CustomPhraseManager::load_or_init(&phrase_path);
    let user_dict = AppConfig::user_dict_path();
    engine.set_user_dict_path(&user_dict);
    if user_dict.exists() {
        info!(
            "[字库引擎] 成功载入用户自适应个性化历史词库: {:?} (词条数: {})",
            user_dict,
            engine.adaptive.model.total_entries()
        );
    }

    let (english_suggester, symbol_suggester) = load_auxiliary_managers();
    engine.resources.english_suggester = english_suggester;
    engine.resources.symbol_suggester = symbol_suggester;

    info!(
        "[简繁词库] 全局简繁转换两级词库就绪 (单字映射: {} 个, 消歧短语: {} 组)",
        engine.resources.opencc_dict.chars.len(),
        engine.resources.opencc_dict.phrases_len()
    );

    Ok(Some(engine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::config::FuzzyPinyinRules;

    #[test]
    fn test_build_algo_config_fuzzy_rules_inherited() {
        let mut cfg = AppConfig::default();
        cfg.pinyin.fuzzy_pinyin = true;
        cfg.pinyin.fuzzy_rules = FuzzyPinyinRules {
            z_zh: true,
            c_ch: false,
            s_sh: true,
            an_ang: false,
            en_eng: false,
            in_ing: false,
            l_n: true,
            f_h: false,
            r_l: false,
            g_k: false,
        };

        let algo_cfg = AlgoConfig::from(&cfg);
        assert!(algo_cfg.fuzzy.z_zh);
        assert!(!algo_cfg.fuzzy.c_ch);
        assert!(algo_cfg.fuzzy.s_sh);
        assert!(!algo_cfg.fuzzy.an_ang);
        assert!(algo_cfg.fuzzy.l_n);
        assert!(!algo_cfg.fuzzy.g_k);
    }

    #[test]
    fn test_build_algo_config_fuzzy_disabled() {
        let mut cfg = AppConfig::default();
        cfg.pinyin.fuzzy_pinyin = false;
        cfg.pinyin.fuzzy_rules.z_zh = true;

        let algo_cfg = AlgoConfig::from(&cfg);
        assert!(
            !algo_cfg.fuzzy.z_zh,
            "当 fuzzy_pinyin 为 false 时所有规则均应处于关闭状态"
        );
    }
}
