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
use waylan_core::input::sync_mode_from_shared_flags;
use waylan_core::input::MOD_CAPS;
use waylan_core::state::SharedInputFlags;
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
    ShortcutToggleAction,
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
        self.core.punctuation_processor.slash_to_dunhao = cfg.pinyin.slash_to_dunhao;
        self.core.punctuation_processor.backslash_to_dunhao = cfg.pinyin.backslash_to_dunhao;
        self.core.punctuation_processor.protect_digits_punctuation =
            cfg.pinyin.protect_digits_punctuation;
        self.core.punctuation_processor.auto_complete_pairs = cfg.pinyin.auto_complete_pairs;
        self.core.punctuation_processor.full_width_punct = cfg.pinyin.full_width_punct;
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

    /// 同步 CapsLock 大写锁定状态至共享标志并唤醒托盘刷新
    pub fn update_caps_lock_state(&mut self, is_locked: bool) {
        if self.core.is_caps_locked() != is_locked {
            if is_locked {
                self.core.modifiers.locked |= MOD_CAPS;
            } else {
                self.core.modifiers.locked &= !MOD_CAPS;
            }
            if let Some(ref flags) = self.shared_flags {
                flags.set_caps_locked(is_locked);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
        }
    }

    /// 同步来自系统托盘的共享中英文状态标志
    pub fn sync_shared_mode(&mut self) -> Option<InputMode> {
        let changed = sync_mode_from_shared_flags(self.shared_flags.as_ref(), &mut self.core.mode);
        if let Some(new_mode) = changed {
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
            ImeOutputAction::ShortcutToggled(toggle) => {
                if let Some(ref flags) = self.shared_flags {
                    match toggle {
                        ShortcutToggleAction::Traditional(v) => flags.set_traditional(v),
                        ShortcutToggleAction::FullWidthChar(v) => flags.set_full_width_char(v),
                        ShortcutToggleAction::FullWidthPunct(v) => flags.set_full_width_punct(v),
                    }
                }
                if let Some(ref tx) = self.mode_notify_tx {
                    let _ = tx.try_send(());
                }
                (true, None)
            }
            ImeOutputAction::ModeChanged {
                new_mode,
                committed,
                was_consumed,
            } => {
                if let Some(ref flags) = self.shared_flags {
                    flags.set_chinese(new_mode == InputMode::Chinese);
                }
                if let Some(ref tx) = self.mode_notify_tx {
                    let _ = tx.try_send(());
                }
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
                inline_preedit,
                voice_manager: self.voice_manager.as_ref(),
            },
        )
    }
}
