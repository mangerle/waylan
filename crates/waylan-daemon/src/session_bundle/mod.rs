//! 会话环境依赖包与热重载管理模块
//!
//! 负责整合输入法会话运行所需的词库加载、配置装配、环境能力探测与 Wayland 原生语音快捷键拦截器。

mod config_bundle;
mod dict_loader;
mod engine_builder;
mod ibus_dispatcher;
mod system_env;
mod voice_filter;
mod wayland_dispatcher;

#[cfg(test)]
pub use config_bundle::SessionConfigBundle;
pub use engine_builder::build_algo_engine;
pub use ibus_dispatcher::{dispatch_ibus_session, IbusDispatchOptions};
pub use system_env::{check_wayland_capabilities, trigger_graceful_restart_and_exit};
pub use wayland_dispatcher::{dispatch_wayland_session, WaylandDispatchOptions};

#[cfg(test)]
mod tests {
    use super::dict_loader::{load_auxiliary_dict, load_lexicon_table, load_opencc_dict};
    use super::voice_filter::build_wayland_voice_manager;
    use super::*;
    use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine, ToggleKeyMode};
    use waylan_algo::{AlgoConfig, AlgoEngine};
    use waylan_core::VoiceConfig;
    use waylan_wayland::session_v1::PageKeyConfig;

    #[test]
    fn test_session_config_bundle_load() {
        let bundle = SessionConfigBundle::load_current();
        assert!(bundle.algo_config.page_size >= 5 && bundle.algo_config.page_size <= 9);
        assert!(bundle.font_size >= 12.0 && bundle.font_size <= 24.0);
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
        let mut engine = AlgoEngine::with_sample_data(AlgoConfig::default());
        let mut shift_sm = ShiftToggleStateMachine::with_toggle_key(ToggleKeyMode::Shift);
        let mut punct = PunctuationProcessor::with_options(true, true, false);
        let mut page_keys = PageKeyConfig::default();

        // 模拟已输入左双引号与上下文网址前缀
        punct.record_commit("“https://example.com");
        assert!(punct.double_quote_open);
        assert_eq!(punct.recent_context, "“https://example.com");

        let mut bundle = SessionConfigBundle::load_current();
        bundle.punct_processor = PunctuationProcessor::with_options(false, true, false);
        bundle.apply_to_engine_and_input(&mut engine, &mut shift_sm, &mut punct, &mut page_keys);

        // 验证热重载更新了配置开关，同时完整保留了引号开闭状态与最近输入上下文
        assert!(!punct.slash_to_dunhao);
        assert!(punct.double_quote_open);
        assert_eq!(punct.recent_context, "“https://example.com");
    }

    #[test]
    fn test_build_wayland_voice_manager_lifecycle() {
        let cfg_disabled = VoiceConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(build_wayland_voice_manager(&cfg_disabled).is_none());

        let cfg_no_shortcuts = VoiceConfig {
            enabled: true,
            push_to_talk_shortcut: String::new(),
            toggle_shortcut: String::new(),
            ..Default::default()
        };
        assert!(build_wayland_voice_manager(&cfg_no_shortcuts).is_none());

        let cfg_enabled = VoiceConfig {
            enabled: true,
            push_to_talk_shortcut: "F8".to_string(),
            ..Default::default()
        };
        let vm = build_wayland_voice_manager(&cfg_enabled);
        assert!(vm.is_some());
        assert_eq!(vm.unwrap().config.push_to_talk_shortcut, "F8");
    }

    #[test]
    fn test_apply_wayland_session_reload_updates_voice_config() {
        use super::config_bundle::apply_reload_v2;
        use waylan_core::config::AppConfig;
        use waylan_wayland::session_v2::WaylanV2Session;
        let engine = AlgoEngine::with_sample_data(Default::default());
        let mut session = WaylanV2Session::new(engine);

        let initial_voice = VoiceConfig {
            enabled: false,
            ..Default::default()
        };
        session.voice_manager = Some(waylan_voice::VoiceInteractionManager::new(initial_voice));

        apply_reload_v2(&mut session);

        // 验证热重载后，Wayland 会话内部的 voice_manager 配置已同步为磁盘当前配置
        let disk_cfg = AppConfig::load_or_create();
        assert_eq!(
            session.voice_manager.as_ref().unwrap().config.enabled,
            disk_cfg.voice.enabled
        );
    }
}
