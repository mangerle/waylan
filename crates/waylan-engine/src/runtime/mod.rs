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
use std::time::{SystemTime, UNIX_EPOCH};

use waylan_algo::AlgoEngine;
use waylan_core::input::MOD_CAPS;
use waylan_core::state::{InputStateSnapshot, SharedInputFlags};
use waylan_core::{CandidateBoxState, InputMode, ParsedShortcut, VoiceConfig};
#[cfg(feature = "voice")]
use waylan_voice::{VoiceInteractionManager, VoiceKeyAction};

#[cfg(not(feature = "voice"))]
pub mod voice_stub;
#[cfg(not(feature = "voice"))]
pub use voice_stub::{VoiceInteractionManager, VoiceKeyAction};

use crate::frontend::InputMethodFrontend;
use crate::session::{
    extract_surrounding_prefix, ImeCoreSession, ImeOutputAction, KeyProcessContext,
};

pub mod candidate_builder;
pub use candidate_builder::{build_common_candidate_box_state, CandidateBoxBuildOptions};

/// 统一按键事件输入参数对象
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessKeyEventParams<'a> {
    pub key: u32,
    pub is_press: bool,
    pub is_password_or_locked: bool,
    pub surrounding_text: Option<&'a (String, u32, u32)>,
    pub inline_preedit: bool,
}

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

    /// 重置会话与焦点上下文（在失焦 FocusOut 或客户端重置时调用）
    pub fn reset_context<F: InputMethodFrontend>(&mut self, frontend: &mut F) {
        if let Some(ref mut vm) = self.voice_manager {
            if vm.is_recording() {
                vm.cancel();
            }
            vm.clear_state();
        }
        self.core.reset_input_state();
        frontend.hide_candidates();
        frontend.update_preedit("", 0);
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

    /// 从底层权威核心状态提取当前完整的不可变输入状态快照
    #[inline]
    pub fn state_snapshot(&self) -> InputStateSnapshot {
        InputStateSnapshot {
            is_chinese: matches!(self.core.mode, InputMode::Chinese | InputMode::VMode),
            is_caps_locked: self.core.is_caps_locked(),
            traditional_chinese: self.core.engine.config.traditional_chinese,
            full_width_punct: self.core.punctuation_processor.is_full_width_punct(),
            full_width_char: self.core.full_width_char,
        }
    }

    /// 向外部共享标志容器与系统托盘单向广播最新状态快照（单一真实源广播出口）
    pub fn broadcast_state_snapshot(&self) {
        let snap = self.state_snapshot();
        if let Some(ref flags) = self.shared_flags {
            flags.update_from_snapshot(&snap);
        }
        if let Some(ref tx) = self.mode_notify_tx {
            let _ = tx.try_send(());
        }
    }

    /// 设置输入语言模式并单向广播状态快照
    pub fn set_input_mode(&mut self, mode: InputMode) {
        if self.core.mode != mode {
            self.core.mode = mode;
            self.broadcast_state_snapshot();
        }
    }

    /// 切换中英文模式并单向广播状态快照
    pub fn toggle_input_mode(&mut self) -> InputMode {
        let next = match self.core.mode {
            InputMode::Chinese | InputMode::VMode => InputMode::English,
            InputMode::English => InputMode::Chinese,
        };
        self.set_input_mode(next);
        next
    }

    /// 设置简繁输出模式并单向广播状态快照
    pub fn set_traditional(&mut self, val: bool) {
        if self.core.engine.config.traditional_chinese != val {
            self.core.engine.config.traditional_chinese = val;
            self.broadcast_state_snapshot();
        }
    }

    /// 切换简繁输出模式并单向广播状态快照
    pub fn toggle_traditional(&mut self) -> bool {
        let next = !self.core.engine.config.traditional_chinese;
        self.set_traditional(next);
        next
    }

    /// 设置标点全半角并单向广播状态快照
    pub fn set_full_width_punct(&mut self, val: bool) {
        if self.core.punctuation_processor.is_full_width_punct() != val {
            self.core.punctuation_processor.set_full_width_punct(val);
            self.broadcast_state_snapshot();
        }
    }

    /// 切换标点全半角并单向广播状态快照
    pub fn toggle_full_width_punct(&mut self) -> bool {
        let next = !self.core.punctuation_processor.is_full_width_punct();
        self.set_full_width_punct(next);
        next
    }

    /// 设置字符全半角并单向广播状态快照
    pub fn set_full_width_char(&mut self, val: bool) {
        if self.core.full_width_char != val {
            self.core.full_width_char = val;
            self.broadcast_state_snapshot();
        }
    }

    /// 切换字符全半角并单向广播状态快照
    pub fn toggle_full_width_char(&mut self) -> bool {
        let next = !self.core.full_width_char;
        self.set_full_width_char(next);
        next
    }

    /// 同步 CapsLock 大写锁定状态至核心状态机并向外部单向广播快照
    pub fn update_caps_lock_state(&mut self, is_locked: bool) {
        if self.core.is_caps_locked() != is_locked {
            if is_locked {
                self.core.modifiers.locked |= MOD_CAPS;
            } else {
                self.core.modifiers.locked &= !MOD_CAPS;
            }
            self.broadcast_state_snapshot();
        }
    }

    /// 同步来自系统托盘的共享中英文、大写锁定、简繁及全半角状态标志
    pub fn sync_shared_mode(&mut self) -> Option<InputMode> {
        let mut mode_changed = None;
        if let Some(ref flags) = self.shared_flags {
            let snap = flags.snapshot();

            // 1. 同步 CapsLock 大写锁定状态
            if self.core.is_caps_locked() != snap.is_caps_locked {
                if snap.is_caps_locked {
                    self.core.modifiers.locked |= MOD_CAPS;
                } else {
                    self.core.modifiers.locked &= !MOD_CAPS;
                }
            }

            // 2. 同步中英文输入语言模式
            let current_zh = matches!(self.core.mode, InputMode::Chinese | InputMode::VMode);
            if snap.is_chinese != current_zh {
                let new_mode = if snap.is_chinese {
                    InputMode::Chinese
                } else {
                    InputMode::English
                };
                self.core.mode = new_mode;
                mode_changed = Some(new_mode);
            }

            // 3. 同步简繁输出模式
            if self.core.engine.config.traditional_chinese != snap.traditional_chinese {
                self.core.engine.config.traditional_chinese = snap.traditional_chinese;
            }

            // 4. 同步全角/半角标点
            if self.core.punctuation_processor.is_full_width_punct() != snap.full_width_punct {
                self.core
                    .punctuation_processor
                    .set_full_width_punct(snap.full_width_punct);
            }

            // 5. 同步全角/半角字符
            if self.core.full_width_char != snap.full_width_char {
                self.core.full_width_char = snap.full_width_char;
            }
        }

        if let Some(new_mode) = mode_changed {
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
            return Some(new_mode);
        }

        None
    }

    /// 统一按键事件驱动主流程
    ///
    /// # 设计原理
    /// - 依次经过：1. 语音按键拦截与长按流转 -> 2. 核心状态机决策 -> 3. 前端回调通知。
    /// - 彻底消灭 Wayland 与 IBus 前端在按键分发与文本提交上的重复逻辑。
    pub fn process_key_event<F: InputMethodFrontend>(
        &mut self,
        frontend: &mut F,
        params: ProcessKeyEventParams<'_>,
    ) -> (bool, Option<String>) {
        let ProcessKeyEventParams {
            key,
            is_press,
            is_password_or_locked,
            surrounding_text,
            inline_preedit,
        } = params;

        // 1. 统一语音按键状态机优先分发
        if let Some(ref mut vm) = self.voice_manager {
            match vm.dispatch_voice_key(
                key,
                is_press,
                self.core.mods_depressed(),
                &mut self.core.consumed_keys,
            ) {
                VoiceKeyAction::Consumed => {
                    self.refresh_frontend_ui(frontend, inline_preedit);
                    return (true, None);
                }
                VoiceKeyAction::Commit(text) => {
                    frontend.commit_text(&text);
                    self.core.punctuation_processor.record_commit(&text);
                    self.refresh_frontend_ui(frontend, inline_preedit);
                    return (true, Some(text));
                }
                VoiceKeyAction::PassThrough => {}
            }
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let context_prefix = extract_surrounding_prefix(surrounding_text, 64);
        let ctx = KeyProcessContext {
            keycode: key,
            is_press,
            is_password_or_locked,
            surrounding_prefix: context_prefix,
        };

        // 2. 调用核心决策状态机
        let result = self.core.process_key(ctx, now);

        // 3. 翻译并消费决策动作
        match result {
            ImeOutputAction::PassThrough => (false, None),
            ImeOutputAction::Handled { update_ui } => {
                if update_ui {
                    self.refresh_frontend_ui(frontend, inline_preedit);
                }
                (true, None)
            }
            ImeOutputAction::ExpandGrid => {
                self.refresh_frontend_ui(frontend, inline_preedit);
                (true, None)
            }
            ImeOutputAction::Commit(text) => {
                frontend.commit_text(&text);
                self.core.punctuation_processor.record_commit(&text);
                self.refresh_frontend_ui(frontend, inline_preedit);
                (true, Some(text))
            }
            ImeOutputAction::CommitAndPassThrough(text) => {
                frontend.commit_text(&text);
                self.core.punctuation_processor.record_commit(&text);
                self.refresh_frontend_ui(frontend, inline_preedit);
                (false, Some(text))
            }
            ImeOutputAction::CommitPunctuation {
                text,
                cursor_offset,
            } => {
                frontend.commit_text(&text);
                self.core.punctuation_processor.record_commit(&text);
                if cursor_offset != 0 {
                    frontend.move_cursor(cursor_offset);
                }
                self.refresh_frontend_ui(frontend, inline_preedit);
                (true, Some(text))
            }
            ImeOutputAction::CommitPartial {
                committed,
                remaining_preedit: _,
            } => {
                frontend.commit_text(&committed);
                self.core.punctuation_processor.record_commit(&committed);
                self.refresh_frontend_ui(frontend, inline_preedit);
                (true, Some(committed))
            }
            ImeOutputAction::Reset => {
                self.core.reset_input_state();
                frontend.hide_candidates();
                frontend.update_preedit("", 0);
                (true, None)
            }
            ImeOutputAction::ShortcutToggled(_toggle) => {
                self.broadcast_state_snapshot();
                (true, None)
            }
            ImeOutputAction::ModeChanged {
                new_mode,
                committed,
                was_consumed,
            } => {
                self.broadcast_state_snapshot();
                frontend.sync_input_mode(new_mode);
                if let Some(ref text) = committed {
                    frontend.commit_text(text);
                }
                self.refresh_frontend_ui(frontend, inline_preedit);
                (was_consumed, committed)
            }
        }
    }

    /// 轮询语音后台事件流并在必要时通知前端更新或提交
    pub fn poll_voice_cycle<F: InputMethodFrontend>(
        &mut self,
        frontend: &mut F,
        inline_preedit: bool,
    ) -> Option<String> {
        if let Some(ref mut vm) = self.voice_manager {
            if vm.is_active() {
                let commit_opt = vm.poll_events();
                if let Some(ref text) = commit_opt {
                    frontend.commit_text(text);
                    self.core.punctuation_processor.record_commit(text);
                    self.refresh_frontend_ui(frontend, inline_preedit);
                    return commit_opt;
                } else {
                    self.refresh_frontend_ui(frontend, inline_preedit);
                }
            }
        }
        None
    }

    /// 统一刷新前端 UI（包括客户端预编辑与候选框卡片）
    pub fn refresh_frontend_ui<F: InputMethodFrontend>(
        &mut self,
        frontend: &mut F,
        inline_preedit: bool,
    ) {
        let is_voice = self.is_voice_active();
        let is_composing = self.is_composing();

        if !is_voice && !is_composing {
            frontend.hide_candidates();
            frontend.update_preedit("", 0);
            return;
        }

        // 1. 同步预编辑字符串给客户端
        let preedit_text = if is_composing {
            self.core.engine.formatted_preedit()
        } else {
            String::new()
        };
        let cursor_pos = if is_composing {
            self.core.engine.cursor()
        } else {
            0
        };
        frontend.update_preedit(&preedit_text, cursor_pos);

        // 2. 构建候选框状态并提交给前端呈现
        let box_state = self.build_candidate_box_state(true, inline_preedit);
        frontend.update_candidates(&box_state);
    }

    /// 构建不可变候选框渲染快照
    pub fn build_candidate_box_state(
        &self,
        is_active: bool,
        inline_preedit: bool,
    ) -> CandidateBoxState {
        build_common_candidate_box_state(
            &self.core.engine,
            CandidateBoxBuildOptions {
                mode: self.core.mode,
                is_active,
                grid_state: self.core.grid_state,
                expanded_card: Some(self.core.expanded_card),
                inline_preedit,
                voice_manager: self.voice_manager.as_ref(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_shared_mode_comprehensive() {
        let mut runtime = ImeRuntime::default();
        // 初始托盘 flags：中文模式、繁体关闭、半角标点、半角字符
        let flags = SharedInputFlags::new(true, false, false, false);
        runtime = runtime.with_shared_flags(flags.clone());

        // 初始状态验证：引擎默认中文全角标点，尚未同步
        assert_eq!(runtime.core.mode, InputMode::Chinese);
        assert!(!runtime.core.engine.config.traditional_chinese);
        assert!(runtime.core.punctuation_processor.is_full_width_punct());
        assert!(!runtime.core.full_width_char);

        // 首次同步：将托盘的半角标点同步至运行时
        let mode_changed = runtime.sync_shared_mode();
        assert_eq!(mode_changed, None);
        assert!(!runtime.core.punctuation_processor.is_full_width_punct());

        // 外部托盘切换为繁体、全角标点、全角字符、英文
        flags.set_traditional(true);
        flags.set_full_width_punct(true);
        flags.set_full_width_char(true);
        flags.set_chinese(false);

        let mode_changed = runtime.sync_shared_mode();
        assert_eq!(mode_changed, Some(InputMode::English));
        assert_eq!(runtime.core.mode, InputMode::English);
        assert!(runtime.core.engine.config.traditional_chinese);
        assert!(runtime.core.punctuation_processor.is_full_width_punct());
        assert!(runtime.core.full_width_char);

        // 再次同步，无状态变化
        let mode_changed = runtime.sync_shared_mode();
        assert_eq!(mode_changed, None);

        // 外部托盘切回简体和中文
        flags.set_traditional(false);
        flags.set_chinese(true);
        let mode_changed = runtime.sync_shared_mode();
        assert_eq!(mode_changed, Some(InputMode::Chinese));
        assert!(!runtime.core.engine.config.traditional_chinese);
    }

    #[test]
    fn test_ime_runtime_single_source_of_truth() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let flags = SharedInputFlags::new(true, false, true, false);
        let mut runtime = ImeRuntime::default()
            .with_shared_flags(flags.clone())
            .with_mode_notify_tx(tx);

        // 验证初始状态一致性
        let snap = runtime.state_snapshot();
        assert!(snap.is_chinese);
        assert!(!snap.is_caps_locked);
        assert_eq!(flags.snapshot(), snap);

        // 切换中英文模式并广播
        let mode = runtime.toggle_input_mode();
        assert_eq!(mode, InputMode::English);
        assert!(!flags.is_chinese());
        assert_eq!(runtime.state_snapshot(), flags.snapshot());
        assert!(rx.try_recv().is_ok());

        // 切换简繁
        let trad = runtime.toggle_traditional();
        assert!(trad);
        assert!(flags.is_traditional());
        assert_eq!(runtime.state_snapshot(), flags.snapshot());
        assert!(rx.try_recv().is_ok());

        // 切换标点全半角
        let punct = runtime.toggle_full_width_punct();
        assert!(!punct);
        assert!(!flags.is_full_width_punct());
        assert_eq!(runtime.state_snapshot(), flags.snapshot());
        assert!(rx.try_recv().is_ok());

        // 切换字符全半角
        let char_full = runtime.toggle_full_width_char();
        assert!(char_full);
        assert!(flags.is_full_width_char());
        assert_eq!(runtime.state_snapshot(), flags.snapshot());
        assert!(rx.try_recv().is_ok());

        // 更新 CapsLock 大写锁定
        runtime.update_caps_lock_state(true);
        assert!(flags.is_caps_locked());
        assert_eq!(runtime.state_snapshot(), flags.snapshot());
        assert!(rx.try_recv().is_ok());
    }
}
