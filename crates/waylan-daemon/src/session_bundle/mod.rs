//! 会话环境依赖包与热重载管理模块
//!
//! 负责整合输入法会话运行所需的词库加载、配置装配、环境能力探测与 Wayland 原生语音快捷键拦截器。

mod engine_builder;
mod ibus_dispatcher;
mod system_env;
mod wayland_dispatcher;

pub use engine_builder::build_algo_engine;
pub use ibus_dispatcher::{IbusDispatchOptions, dispatch_ibus_session};
pub use system_env::{check_wayland_capabilities, trigger_graceful_restart_and_exit};
pub use wayland_dispatcher::{WaylandDispatchOptions, dispatch_wayland_session};

use std::sync::atomic::{AtomicBool, Ordering};

use waylan_core::config::AppConfig;
use waylan_core::input::VoiceProvider;

use crate::desktop::{
    GnomeExtensionIntent, ensure_gnome_input_source_registered, gnome_extension_intent,
    sync_gnome_extension_state_async, sync_gnome_hide_indicator_extension,
};
use crate::system::ShutdownCoordinator;
use crate::tray::TraySyncHandle;

/// 构建双后端共用的统一语音交互适配器
pub(super) fn build_voice_provider(cfg: &AppConfig) -> Option<Box<dyn VoiceProvider>> {
    Some(Box::new(waylan_voice::VoiceInteractionManager::new(
        cfg.voice.clone(),
    )))
}

/// 宿主热重载协作桥接器构建参数
pub(super) struct HostReloadBridgeOptions<'a> {
    pub cfg: &'a AppConfig,
    pub has_tray: bool,
    pub use_ibus_backend: bool,
    pub coordinator: ShutdownCoordinator,
    pub tray_sync: Option<TraySyncHandle>,
}

/// 双后端共享的宿主级热重载桥接器
///
/// # 设计原理
/// - **实现初衷**：过去 Wayland 会话重载路径只更新引擎与渲染器，遗漏托盘展示形态变更重启判定、
///   托盘自定义图标热更新与 GNOME Shell 扩展状态同步，导致双后端运行语义分裂。
/// - **核心优势**：统一收敛托盘重启判定、图标实时同步与 GNOME 扩展变迁追踪，消除双后端行为漂移。
pub(super) struct HostReloadBridge {
    has_tray: bool,
    use_ibus_backend: bool,
    coordinator: ShutdownCoordinator,
    tray_sync: Option<TraySyncHandle>,
    hide_indicator: AtomicBool,
    custom_candidate_view: AtomicBool,
    pending_desktop_sync: AtomicBool,
}

impl HostReloadBridge {
    /// 基于启动期配置构造热重载桥接器
    pub(super) fn new(opts: HostReloadBridgeOptions<'_>) -> Self {
        let initial_intent = gnome_extension_intent(opts.cfg);
        Self {
            has_tray: opts.has_tray,
            use_ibus_backend: opts.use_ibus_backend,
            coordinator: opts.coordinator,
            tray_sync: opts.tray_sync,
            hide_indicator: AtomicBool::new(initial_intent.hide_indicator),
            custom_candidate_view: AtomicBool::new(initial_intent.custom_candidate_view),
            pending_desktop_sync: AtomicBool::new(false),
        }
    }

    /// 获取当前缓存的 GNOME 扩展意图
    pub(super) fn current_intent(&self) -> GnomeExtensionIntent {
        GnomeExtensionIntent {
            hide_indicator: self.hide_indicator.load(Ordering::Acquire),
            custom_candidate_view: self.custom_candidate_view.load(Ordering::Acquire),
        }
    }

    fn update_intent(&self, intent: GnomeExtensionIntent) -> bool {
        let prev_hide = self
            .hide_indicator
            .swap(intent.hide_indicator, Ordering::AcqRel);
        let prev_view = self
            .custom_candidate_view
            .swap(intent.custom_candidate_view, Ordering::AcqRel);
        prev_hide != intent.hide_indicator || prev_view != intent.custom_candidate_view
    }

    /// 执行宿主级热重载校验与托盘同步；若需平滑重启守护进程则返回 `true`
    pub(super) fn handle_reload(
        &self,
        new_cfg: &AppConfig,
        trigger_restart_directly: bool,
        async_desktop_sync: bool,
    ) -> bool {
        if should_restart_for_tray_change(new_cfg, self.has_tray, self.use_ibus_backend) {
            if trigger_restart_directly {
                self.coordinator.trigger_graceful_restart();
            }
            return true;
        }

        if let Some(ref sync) = self.tray_sync {
            sync.sync_config(new_cfg);
        }

        let next_intent = gnome_extension_intent(new_cfg);
        if self.update_intent(next_intent) {
            if async_desktop_sync {
                sync_gnome_extension_state_async(next_intent);
            } else {
                self.pending_desktop_sync.store(true, Ordering::Release);
            }
        }
        false
    }

    /// 在外层同步锁完全释放后执行延迟的桌面扩展刷新操作
    pub(super) fn flush_pending_desktop_sync(&self) {
        if self.pending_desktop_sync.swap(false, Ordering::AcqRel) {
            let intent = self.current_intent();
            ensure_gnome_input_source_registered(intent.hide_indicator);
            sync_gnome_hide_indicator_extension(intent);
        }
    }
}

/// 判定配置热重载后托盘展示形态是否发生改变（决定是否需要优雅重启以重建托盘）
#[inline]
pub(super) fn should_restart_for_tray_change(
    new_cfg: &AppConfig,
    current_has_tray: bool,
    use_ibus_backend: bool,
) -> bool {
    crate::system::determine_tray_should_spawn(new_cfg, use_ibus_backend) != current_has_tray
}

/// 统一收敛协议后端事件循环退出结果（屏蔽停机期间的预期中断错误）
pub(super) fn finalize_session_loop_result<E>(
    loop_result: Result<(), E>,
    coordinator: &crate::system::ShutdownCoordinator,
    backend_label: &str,
) -> Result<(), crate::error::DaemonError>
where
    E: Into<crate::error::DaemonError>,
{
    match loop_result {
        Ok(_) => {
            log::info!(
                "[微澜守护进程] {} 输入法事件循环已正常退出，所有资源已安全释放。",
                backend_label
            );
            Ok(())
        }
        Err(_) if coordinator.is_shutting_down() => {
            log::info!(
                "[微澜守护进程] 收到退出信号或托盘退出请求，{} 服务安全退场。",
                backend_label
            );
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use waylan_algo::{AlgoConfig, AlgoEngine};
    use waylan_core::VoiceConfig;
    use waylan_core::config::AppConfig;
    use waylan_engine::{ImeRuntime, load_auxiliary_dict, load_lexicon_table, load_opencc_dict};

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
            .punctuation_processor_mut()
            .record_commit("“https://example.com");
        assert!(runtime.core.punctuation_processor().double_quote_open);
        assert_eq!(
            runtime.core.punctuation_processor().recent_context,
            "“https://example.com"
        );

        let mut cfg = AppConfig::load_or_create();
        cfg.pinyin.slash_to_dunhao = false;
        cfg.pinyin.backslash_to_dunhao = false;
        runtime.reload_config(&cfg);

        // 验证热重载更新了配置开关，同时完整保留了引号开闭状态与最近输入上下文
        assert!(!runtime.core.punctuation_processor().options.slash_to_dunhao);
        assert!(
            !runtime
                .core
                .punctuation_processor()
                .options
                .backslash_to_dunhao
        );
        assert!(runtime.core.punctuation_processor().double_quote_open);
        assert_eq!(
            runtime.core.punctuation_processor().recent_context,
            "“https://example.com"
        );
    }

    #[test]
    fn test_apply_wayland_session_reload_updates_voice_config() {
        use waylan_core::config::AppConfig;
        use waylan_wayland::reload_wayland_session;
        use waylan_wayland::session_v2::WaylanV2Session;
        let engine = AlgoEngine::with_sample_data(Default::default());
        let mut session = WaylanV2Session::new(engine);

        let initial_voice = VoiceConfig {
            enabled: false,
            ..Default::default()
        };
        session.runtime.set_voice_provider(Some(Box::new(
            waylan_voice::VoiceInteractionManager::new(initial_voice),
        )));

        reload_wayland_session(&mut session);

        // 验证热重载后，Wayland 会话内部的 voice_manager 配置已同步为磁盘当前配置
        let disk_cfg = AppConfig::load_or_create();
        assert_eq!(
            session
                .runtime
                .voice_manager()
                .expect("语音管理器应存在")
                .is_enabled(),
            disk_cfg.voice.enabled
        );
    }

    #[test]
    fn test_apply_wayland_v1_session_reload() {
        use waylan_wayland::reload_wayland_session;
        use waylan_wayland::session_v1::WaylanV1Session;
        let engine = AlgoEngine::with_sample_data(Default::default());
        let mut session = WaylanV1Session::with_engine(engine);

        reload_wayland_session(&mut session);

        let disk_cfg = AppConfig::load_or_create();
        assert_eq!(
            session.runtime.core.engine.config.page_size,
            disk_cfg.ui.page_size
        );
    }
}
