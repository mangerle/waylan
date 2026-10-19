//! 统一按键事件驱动与动作处理模块
//!
//! # 设计原理
//! - **实现初衷**：将按键事件驱动、语音快捷键拦截与核心状态机动作翻译独立为专用调度器。
//! - **核心优势**：依次流转：1. 语音按键拦截与长按流转 -> 2. 核心状态机决策 -> 3. 前端回调通知；
//!   彻底消灭 Wayland 与 IBus 双协议栈在事件派发上的样板代码。
//! - **代价与局限**：动作消费需解构并分发给具体前端实现。

use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "voice")]
use waylan_voice::VoiceKeyAction;

#[cfg(not(feature = "voice"))]
use super::voice_stub::VoiceKeyAction;

use super::types::ProcessKeyEventParams;
use super::ImeRuntime;
use crate::frontend::InputMethodFrontend;
use crate::session::{extract_surrounding_prefix, ImeOutputAction, KeyProcessContext};

impl ImeRuntime {
    /// 统一按键事件驱动主流程
    ///
    /// # 设计原理
    /// - 依次经过：1. 统一语音按键拦截与长按流转 -> 2. 核心状态机决策 -> 3. 前端回调通知。
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
}
