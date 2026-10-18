//! 会话环境依赖包与热重载管理模块
//!
//! 负责整合输入法会话运行所需的词库加载、配置装配、环境能力探测与 Wayland 原生语音快捷键拦截器。

mod dict_loader;
mod engine_builder;
mod ibus_dispatcher;
mod system_env;
mod wayland_dispatcher;

pub use engine_builder::build_algo_engine;
pub use ibus_dispatcher::{dispatch_ibus_session, IbusDispatchOptions};
pub use system_env::{check_wayland_capabilities, trigger_graceful_restart_and_exit};
pub use wayland_dispatcher::{dispatch_wayland_session, WaylandDispatchOptions};

#[cfg(test)]
pub use wayland_dispatcher::{reload_wayland_v1_session, reload_wayland_v2_session};

#[cfg(test)]
mod tests {
    use super::dict_loader::{load_auxiliary_dict, load_lexicon_table, load_opencc_dict};
    use super::*;
    use waylan_algo::{AlgoConfig, AlgoEngine};
    use waylan_core::config::AppConfig;
    use waylan_core::VoiceConfig;
    use waylan_engine::ImeRuntime;

    #[test]
    fn test_app_config_to_algo_config_conversion() {
        let cfg = AppConfig::load_or_create();
        let algo_cfg = AlgoConfig::from(&cfg);
        assert!(algo_cfg.page_size >= 5 && algo_cfg.page_size <= 9);
        assert!(cfg.ui.font_size >= 12.0 && cfg.ui.font_size <= 24.0);
    }

    #[test]
    fn test_load_lexicon_table_fallback() {
        let table = load_lexicon_table();
        assert!(!table.is_empty(), "回退词库应包含基础词条数据");
    }

    #[test]
    fn test_load_auxiliary_and_opencc_dict_fallback() {
        let aux = load_auxiliary_dict();
        assert!(!aux.english_words.is_empty(), "辅助词库应包含基础英文词条");
        assert!(!aux.symbols.is_empty(), "辅助词库应包含基础符号映射");

        let opencc = load_opencc_dict();
        assert!(!opencc.chars.is_empty(), "简繁词库应包含基础字符映射");
    }

    #[test]
    fn test_reload_preserves_punctuation_context() {
        let engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut runtime = ImeRuntime::with_engine(engine);

        // 模拟已输入左双引号与上下文网址前缀
        runtime
            .core
            .punctuation_processor
            .record_commit("“https://example.com");
        assert!(runtime.core.punctuation_processor.double_quote_open);
        assert_eq!(
            runtime.core.punctuation_processor.recent_context,
            "“https://example.com"
        );

        let mut cfg = AppConfig::load_or_create();
        cfg.pinyin.slash_to_dunhao = false;
        cfg.pinyin.backslash_to_dunhao = false;
        runtime.reload_config(&cfg);

        // 验证热重载更新了配置开关，同时完整保留了引号开闭状态与最近输入上下文
        assert!(!runtime.core.punctuation_processor.slash_to_dunhao);
        assert!(!runtime.core.punctuation_processor.backslash_to_dunhao);
        assert!(runtime.core.punctuation_processor.double_quote_open);
        assert_eq!(
            runtime.core.punctuation_processor.recent_context,
            "“https://example.com"
        );
    }

    #[test]
    fn test_apply_wayland_session_reload_updates_voice_config() {
        use waylan_core::config::AppConfig;
        use waylan_wayland::session_v2::WaylanV2Session;
        let engine = AlgoEngine::with_sample_data(Default::default());
        let mut session = WaylanV2Session::new(engine);

        let initial_voice = VoiceConfig {
            enabled: false,
            ..Default::default()
        };
        session.runtime.voice_manager =
            Some(waylan_voice::VoiceInteractionManager::new(initial_voice));

        reload_wayland_v2_session(&mut session);

        // 验证热重载后，Wayland 会话内部的 voice_manager 配置已同步为磁盘当前配置
        let disk_cfg = AppConfig::load_or_create();
        assert_eq!(
            session
                .runtime
                .voice_manager
                .as_ref()
                .unwrap()
                .config
                .enabled,
            disk_cfg.voice.enabled
        );
    }

    #[test]
    fn test_apply_wayland_v1_session_reload() {
        use waylan_wayland::session_v1::WaylanV1Session;
        let engine = AlgoEngine::with_sample_data(Default::default());
        let mut session = WaylanV1Session::with_engine(engine);

        reload_wayland_v1_session(&mut session);

        let disk_cfg = AppConfig::load_or_create();
        assert_eq!(
            session.runtime.core.engine.config.page_size,
            disk_cfg.ui.page_size
        );
    }
}
