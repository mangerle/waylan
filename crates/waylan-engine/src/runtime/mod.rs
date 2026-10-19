//! 微澜输入法统一交互运行时与多协议会话引擎 (IME Runtime)
//!
//! # 设计原理
//! - **实现初衷**：彻底收敛 Wayland 与 IBus 双协议栈之间长期存在的按键拦截样板逻辑、语音快捷键处理、
//!   候选词构建与状态同步代码。
//! - **核心优势**：依赖倒置原则（DIP）；协议适配层只需实现 [`crate::frontend::InputMethodFrontend`] 契约，
//!   即可由 `ImeRuntime` 单一真实源驱动所有打字交互、状态流转、预编辑更新与语音会话。
//! - **代价与局限**：协议层需以借用形式提供前端抽象实例。

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use waylan_algo::AlgoEngine;
use waylan_core::state::SharedInputFlags;
use waylan_core::{ParsedShortcut, VoiceConfig};
#[cfg(feature = "voice")]
use waylan_voice::VoiceInteractionManager;

#[cfg(not(feature = "voice"))]
pub mod voice_stub;
#[cfg(not(feature = "voice"))]
pub use voice_stub::VoiceInteractionManager;

pub mod candidate_builder;
pub mod key_dispatcher;
pub mod state_sync;
pub mod types;
pub mod ui_adapter;

#[cfg(test)]
mod tests;

pub use candidate_builder::{build_common_candidate_box_state, CandidateBoxBuildOptions};
pub use types::ProcessKeyEventParams;

use crate::session::ImeCoreSession;

/// 微澜统一输入法交互运行时 (IME Runtime)
pub struct ImeRuntime {
    /// 核心算法与按键决策会话状态机
    pub core: ImeCoreSession,
    /// 统一语音按键交互与录音流式管理器
    pub voice_manager: Option<VoiceInteractionManager>,
    /// 与系统托盘和外部进程共享的原子状态标志集合
    pub shared_flags: Option<SharedInputFlags>,
    /// 切换中英文或大写时唤醒系统托盘刷新图标的通知发送端
    pub mode_notify_tx: Option<SyncSender<()>>,
    /// 与外部系统托盘共享的配置与词库重载请求标志
    pub reload_requested: Option<Arc<AtomicBool>>,
}

impl Default for ImeRuntime {
    fn default() -> Self {
        Self::new(ImeCoreSession::default())
    }
}

impl ImeRuntime {
    /// 使用指定的底层核心会话创建统一交互运行时
    pub fn new(core: ImeCoreSession) -> Self {
        Self {
            core,
            voice_manager: Some(VoiceInteractionManager::new(VoiceConfig::default())),
            shared_flags: None,
            mode_notify_tx: None,
            reload_requested: None,
        }
    }

    /// 使用指定的算法引擎快速构建运行时实例
    pub fn with_engine(engine: AlgoEngine) -> Self {
        Self::new(ImeCoreSession::new(engine))
    }

    /// 挂载与外部共享的输入状态标志
    pub fn with_shared_flags(mut self, flags: SharedInputFlags) -> Self {
        self.shared_flags = Some(flags);
        self
    }

    /// 挂载中英文切换托盘通知发送通道
    pub fn with_mode_notify_tx(mut self, tx: SyncSender<()>) -> Self {
        self.mode_notify_tx = Some(tx);
        self
    }

    /// 挂载配置重载请求标志
    pub fn with_reload_requested(mut self, flag: Arc<AtomicBool>) -> Self {
        self.reload_requested = Some(flag);
        self
    }

    /// 获取核心会话不可变引用
    #[inline]
    pub fn core(&self) -> &ImeCoreSession {
        &self.core
    }

    /// 获取核心会话可变引用
    #[inline]
    pub fn core_mut(&mut self) -> &mut ImeCoreSession {
        &mut self.core
    }

    /// 获取语音管理器不可变引用
    #[inline]
    pub fn voice_manager(&self) -> Option<&VoiceInteractionManager> {
        self.voice_manager.as_ref()
    }

    /// 获取语音管理器可变引用
    #[inline]
    pub fn voice_manager_mut(&mut self) -> Option<&mut VoiceInteractionManager> {
        self.voice_manager.as_mut()
    }

    /// 查询当前是否处于语音激活状态（录音中、等待转录结算中或有展示浮条）
    #[inline]
    pub fn is_voice_active(&self) -> bool {
        self.voice_manager
            .as_ref()
            .map(|v| v.is_active())
            .unwrap_or(false)
    }

    /// 查询当前是否处于输入法激活与候选词/组词展示态
    #[inline]
    pub fn is_composing(&self) -> bool {
        !self.core.engine.raw_input().is_empty()
    }

    /// 热重载应用程序配置与词库资源（使输入法交互运行时自闭环更新）
    pub fn reload_config(&mut self, cfg: &waylan_core::AppConfig) {
        crate::factory::reload_engine_resources(&mut self.core.engine, cfg);

        self.core.shift_sm.set_toggle_key(cfg.shortcuts.switch_key);
        self.core
            .punctuation_processor
            .set_options((&cfg.pinyin).into());
        self.core.page_keys = waylan_core::input::PageKeyConfig::from(cfg.shortcuts.page_keys);
        self.core.traditional_shortcut = ParsedShortcut::parse(&cfg.shortcuts.traditional_shortcut);
        self.core.full_width_char_shortcut =
            ParsedShortcut::parse(&cfg.shortcuts.full_width_char_shortcut);
        self.core.full_width_punct_shortcut =
            ParsedShortcut::parse(&cfg.shortcuts.full_width_punct_shortcut);

        // 语音配置同步
        if let Some(ref mut vm) = self.voice_manager {
            vm.update_config(cfg.voice.clone());
        }
    }
}
