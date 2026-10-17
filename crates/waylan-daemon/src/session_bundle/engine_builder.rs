//! 拼音算法引擎初始化与词库资源装配模块
//!
//! # 设计原理
//! - **实现初衷**：在守护进程启动阶段统一装配全量核心词库、自定义短语表、用户自学习词库与简繁体双向转换表。
//! - **核心优势**：在装配各阶段检测停机中断信号，杜绝冷启动中途收到 SIGTERM/SIGINT 时无法即时退出的缺陷。
//! - **代价与局限**：首次冷启动构建基础字库需要消耗数十毫秒级内存加载时间。

use std::error::Error;

use log::info;
use waylan_algo::custom_phrase::CustomPhraseManager;
use waylan_algo::types::{AlgoConfig, FuzzyConfig};
use waylan_algo::AlgoEngine;
use waylan_core::config::AppConfig;

use super::dict_loader::{load_auxiliary_managers, load_lexicon_table};
use crate::system::ShutdownCoordinator;

/// 构建算法引擎并加载全量汉字库、自定义短语与用户个性化词库
pub fn build_algo_engine(
    cfg: &AppConfig,
    coordinator: &ShutdownCoordinator,
) -> Result<Option<AlgoEngine>, Box<dyn Error>> {
    let table = load_lexicon_table();
    if coordinator.is_shutting_down() {
        info!("[微澜守护进程] 启动加载期间收到终止信号，立即停止启动并安全退场。");
        return Ok(None);
    }
    let algo_config = AlgoConfig {
        page_size: cfg.page_size,
        enable_v_mode: cfg.v_mode,
        enable_u_mode: cfg.enable_u_mode,
        enable_i_mode: cfg.enable_i_mode,
        traditional_chinese: cfg.traditional_chinese,
        filter_uncommon_charset: cfg.filter_uncommon_charset,
        show_pinyin_comment: cfg.show_pinyin_comment,
        auto_dedup: cfg.auto_dedup,
        fuzzy: if cfg.fuzzy_pinyin {
            FuzzyConfig::all_enabled()
        } else {
            FuzzyConfig::default()
        },
        auto_pangu_spacing: cfg.auto_pangu_spacing,
    };
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
