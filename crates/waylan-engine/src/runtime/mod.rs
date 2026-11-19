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
use waylan_core::input::VoiceProvider;
use waylan_core::state::SharedInputFlags;
use waylan_core::{CandidateIndexStyle, CandidateOrientation, ParsedShortcut};

pub mod candidate_builder;
pub mod key_dispatcher;
pub mod state_sync;
pub mod types;
pub mod ui_adapter;
pub mod window_tracker;

#[cfg(test)]
mod tests;

pub use candidate_builder::{build_common_candidate_box_state, CandidateBoxBuildOptions};
pub use types::ProcessKeyEventParams;
pub use window_tracker::WindowModeTracker;

use crate::session::{ImeCoreSession, InputDecorationFlags};

/// 微澜统一输入法交互运行时 (IME Runtime)
pub struct ImeRuntime {
    /// 核心算法与按键决策会话状态机
    pub core: ImeCoreSession,
    /// 窗口级输入模式状态跟踪器（实现多窗口独立记忆）
    pub window_tracker: WindowModeTracker,
    /// 是否启用多窗口输入模式独立记忆
    pub window_mode_memory: bool,
    /// 终端类应用新窗口是否默认使用英文模式
    pub terminal_default_english: bool,
    /// 候选词序号显示风格
    candidate_index_style: CandidateIndexStyle,
    /// 候选框排列方向
    candidate_orientation: CandidateOrientation,
    /// 当前是否处于受保护安全上下文（密码框、PIN 码或系统锁屏）
    is_protected_context: bool,
    /// 统一语音按键交互与录音流式提供者（抽象特型对象）
    voice_manager: Option<Box<dyn VoiceProvider>>,
    /// 与系统托盘和外部进程共享的原子状态标志集合
    shared_flags: Option<SharedInputFlags>,
    /// 切换中英文或大写时唤醒系统托盘刷新图标的通知发送端
    mode_notify_tx: Option<SyncSender<()>>,
    /// 与外部系统托盘共享的配置与词库重载请求标志
    reload_requested: Option<Arc<AtomicBool>>,
}

impl Default for ImeRuntime {
    fn default() -> Self {
        Self::new(ImeCoreSession::default())
    }
}

impl ImeRuntime {
    /// 使用指定的底层核心会话创建统一交互运行时（默认无语音注入）
    pub fn new(core: ImeCoreSession) -> Self {
        Self {
            core,
            window_tracker: WindowModeTracker::default(),
            window_mode_memory: true,
            terminal_default_english: true,
            candidate_index_style: CandidateIndexStyle::default(),
            candidate_orientation: CandidateOrientation::default(),
            is_protected_context: false,
            voice_manager: None,
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

    /// 设置与托盘共享的输入状态标志
    pub fn set_shared_flags(&mut self, flags: Option<SharedInputFlags>) {
        self.shared_flags = flags;
    }

    /// 查询是否挂载了托盘共享状态
    #[inline]
    pub fn has_shared_flags(&self) -> bool {
        self.shared_flags.is_some()
    }

    /// 设置托盘刷新通知通道
    pub fn set_mode_notify_tx(&mut self, tx: Option<SyncSender<()>>) {
        self.mode_notify_tx = tx;
    }

    /// 设置配置重载请求标志
    pub fn set_reload_requested(&mut self, flag: Option<Arc<AtomicBool>>) {
        self.reload_requested = flag;
    }

    /// 获取配置重载请求标志的共享句柄
    pub fn reload_requested(&self) -> Option<Arc<AtomicBool>> {
        self.reload_requested.clone()
    }

    /// 挂载外部语音或多模态输入提供者
    pub fn with_voice_provider(mut self, provider: Box<dyn VoiceProvider>) -> Self {
        self.voice_manager = Some(provider);
        self
    }

    /// 动态设置外部语音输入提供者
    pub fn set_voice_provider(&mut self, provider: Option<Box<dyn VoiceProvider>>) {
        self.voice_manager = provider;
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
    pub fn voice_manager(&self) -> Option<&(dyn VoiceProvider + '_)> {
        self.voice_manager.as_deref()
    }

    /// 获取语音管理器可变引用
    #[inline]
    pub fn voice_manager_mut(&mut self) -> Option<&mut (dyn VoiceProvider + '_)> {
        self.voice_manager
            .as_deref_mut()
            .map(|b| b as &mut (dyn VoiceProvider + '_))
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

    /// 获取候选词序号显示风格
    #[inline]
    pub fn candidate_index_style(&self) -> CandidateIndexStyle {
        self.candidate_index_style
    }

    /// 获取候选框排列方向
    #[inline]
    pub fn candidate_orientation(&self) -> CandidateOrientation {
        self.candidate_orientation
    }

    /// 应用启动阶段的初始配置（仅调用一次）
    ///
    /// 在热重载路径中不能调用本方法，否则每次保存设置都会把用户当前的中英文模式强制重置为
    /// `init_chinese`。热重载请使用 [`ImeRuntime::reload_config`]。
    pub fn apply_initial_config(&mut self, cfg: &waylan_core::AppConfig) {
        self.reload_config(cfg);
        let initial_mode = if cfg.pinyin.init_chinese {
            waylan_core::InputMode::Chinese
        } else {
            waylan_core::InputMode::English
        };
        self.core.set_input_mode(initial_mode);
    }

    /// 热重载应用程序配置与词库资源（使输入法交互运行时自闭环更新）
    pub fn reload_config(&mut self, cfg: &waylan_core::AppConfig) {
        crate::factory::reload_engine_resources(&mut self.core.engine, cfg);

        self.core.shift_sm.set_toggle_key(cfg.shortcuts.switch_key);
        self.core
            .punctuation_processor
            .set_options((&cfg.pinyin).into());
        self.core.set_decoration_flags(InputDecorationFlags::new(
            cfg.pinyin.traditional_chinese,
            cfg.pinyin.full_width_punct,
            cfg.pinyin.full_width_char,
        ));
        self.core.set_page_size(cfg.ui.page_size);
        self.candidate_index_style = cfg.ui.candidate_index_style;
        self.candidate_orientation = cfg.ui.candidate_orientation;
        self.core.page_keys = waylan_core::input::PageKeyConfig::from(cfg.shortcuts.page_keys);
        self.core.shift_select_second = cfg.shortcuts.shift_select_second;
        self.core.traditional_shortcut = ParsedShortcut::parse(&cfg.shortcuts.traditional_shortcut);
        self.core.full_width_char_shortcut =
            ParsedShortcut::parse(&cfg.shortcuts.full_width_char_shortcut);
        self.core.full_width_punct_shortcut =
            ParsedShortcut::parse(&cfg.shortcuts.full_width_punct_shortcut);

        // 桌面与窗口模式配置同步
        self.terminal_default_english = cfg.desktop.terminal_default_english;
        self.window_mode_memory = cfg.desktop.window_mode_memory;

        // 语音配置同步
        if let Some(ref mut vm) = self.voice_manager {
            vm.update_config(&cfg.voice);
        }
    }

    /// 查询当前是否处于受保护安全上下文（密码框或锁屏）
    #[inline]
    pub fn is_protected_context(&self) -> bool {
        self.is_protected_context
    }

    /// 设置受保护安全上下文状态，若发生状态改变则自动单向广播最新状态快照
    ///
    /// # 设计原理
    /// - 进入受保护上下文时清空未提交的拼音缓冲，并向外广播投影后的英文状态；
    /// - 离开受保护上下文时天然露出当前窗口原本记录的基础模式并向外广播。
    pub fn set_protected_context(&mut self, is_protected: bool) -> bool {
        if self.is_protected_context != is_protected {
            self.is_protected_context = is_protected;
            if is_protected && !self.core.engine.raw_input().is_empty() {
                self.core.reset_input_state();
            }
            self.broadcast_state_snapshot();
            true
        } else {
            false
        }
    }

    /// 计算当前在外部指示器与状态决策中生效的实际输入模式（纯函数有效状态投影）
    #[inline]
    pub fn effective_input_mode(&self) -> waylan_core::InputMode {
        if self.is_protected_context {
            waylan_core::InputMode::English
        } else {
            self.core.input_mode()
        }
    }

    /// 窗口获得焦点时切换当前活动上下文并恢复该窗口记录的基础模式
    pub fn focus_window(&mut self, window_id: &str, app_name: Option<&str>) {
        let base_mode = self.window_tracker.focus_window(
            window_id,
            app_name,
            self.terminal_default_english,
            self.window_mode_memory,
        );
        if self.core.input_mode() != base_mode {
            self.core.set_input_mode(base_mode);
        }
        self.broadcast_state_snapshot();
    }

    /// 窗口失去焦点时通知跟踪器
    pub fn blur_window(&mut self, window_id: &str) {
        self.window_tracker.blur_window(window_id);
    }

    /// 窗口销毁时从跟踪器移除
    pub fn remove_window(&mut self, window_id: &str) {
        self.window_tracker.remove_window(window_id);
    }
}
