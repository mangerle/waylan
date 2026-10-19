//! 拼音算法引擎初始化与词库资源装配模块
//!
//! # 设计原理
//! - **实现初衷**：在守护进程启动阶段统一装配全量核心词库、自定义短语表、用户自学习词库与简繁体双向转换表。
//! - **核心优势**：在装配各阶段检测停机中断信号，杜绝冷启动中途收到 SIGTERM/SIGINT 时无法即时退出的缺陷。
//! - **代价与局限**：首次冷启动构建基础字库需要消耗数十毫秒级内存加载时间。

use crate::error::DaemonError;

use crate::system::ShutdownCoordinator;
use log::info;
use waylan_algo::AlgoEngine;
use waylan_core::config::AppConfig;
use waylan_engine::{create_engine_with_table, load_lexicon_table};

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
    let engine = create_engine_with_table(table, cfg);
    Ok(Some(engine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_algo::AlgoConfig;
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
