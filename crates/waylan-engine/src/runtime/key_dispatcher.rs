//! 统一按键事件驱动与动作处理模块
//!
//! # 设计原理
//! - **实现初衷**：将按键事件驱动、语音快捷键拦截与核心状态机动作翻译独立为专用调度器。
//! - **核心优势**：依次流转：1. 语音按键拦截与长按流转 -> 2. 核心状态机决策 -> 3. 前端回调通知；
//!   彻底消灭 Wayland 与 IBus 双协议栈在事件派发上的样板代码。
//! - **代价与局限**：动作消费需解构并分发给具体前端实现。

use std::time::{SystemTime, UNIX_EPOCH};

use waylan_core::InputMode;
use waylan_core::input::{VoiceKeyAction, VoiceKeyDispatchParams};

use super::ImeRuntime;
use super::types::ProcessKeyEventParams;
use crate::frontend::InputMethodFrontend;
use crate::session::{ImeOutputAction, KeyProcessContext, extract_surrounding_prefix};

/// 获取当前 Unix 秒级时间戳（系统时钟异常时回退为 0）
fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 输入法动作派发上下文
///
/// 聚合前端句柄与渲染模式，供按键事件驱动与动作派发辅助函数共享，
/// 避免辅助函数出现过长参数列表。
struct ActionDispatchContext<'a, F: InputMethodFrontend> {
    /// 前端句柄（负责提交文本、刷新界面与隐藏候选框）
    frontend: &'a mut F,
    /// 是否以行内预编辑模式渲染界面
    inline_preedit: bool,
}

impl ImeRuntime {
    /// 统一向前端提交上屏文本并同步登记到标点智能配对处理器
    #[inline]
    fn commit_text_with_record<F: InputMethodFrontend>(&mut self, frontend: &mut F, text: &str) {
        frontend.commit_text(text);
        self.core.punctuation_processor.record_commit(text);
    }

    /// 统一按键事件驱动主流程
    ///
    /// # 设计原理
    /// - 依次经过：1. 统一语音按键拦截与长按流转 -> 2. 核心状态机决策 -> 3. 前端动作派发与界面刷新。
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

        let mut ctx = ActionDispatchContext {
            frontend,
            inline_preedit,
        };

        // 1. 统一语音按键状态机优先分发
        if let Some(result) = self.dispatch_voice_key_event(&mut ctx, key, is_press) {
            return result;
        }

        // 2. 组装核心状态机上下文并执行决策
        let now = current_timestamp_secs();
        let context_prefix = extract_surrounding_prefix(surrounding_text, 64);
        let effective_is_password_or_locked = is_password_or_locked || self.is_protected_context;
        let key_ctx = KeyProcessContext {
            keycode: key,
            is_press,
            is_password_or_locked: effective_is_password_or_locked,
            surrounding_prefix: context_prefix,
        };
        let prev_caps = self.core.is_caps_locked();
        let action = self.core.process_key(key_ctx, now);
        if self.core.is_caps_locked() != prev_caps {
            self.broadcast_state_snapshot();
        }

        // 3. 翻译并消费决策动作
        self.dispatch_ime_action(&mut ctx, action)
    }

    /// 统一语音按键状态机的优先拦截
    ///
    /// # 设计原理
    /// - 语音按键具备最高优先级：一旦被消费或触发文本上屏即刻终止主流程，
    ///   交由核心状态机继续处理时返回 None 以保持原有放行语义。
    fn dispatch_voice_key_event<F: InputMethodFrontend>(
        &mut self,
        ctx: &mut ActionDispatchContext<'_, F>,
        key: u32,
        is_press: bool,
    ) -> Option<(bool, Option<String>)> {
        let modifiers = self.core.mods_depressed();
        let vm = self.voice_manager.as_mut()?;
        match vm.dispatch_key(VoiceKeyDispatchParams {
            key,
            is_press,
            modifiers,
            consumed_keys: &mut self.core.consumed_keys,
        }) {
            VoiceKeyAction::Consumed => {
                self.refresh_frontend_ui(ctx.frontend, ctx.inline_preedit);
                Some((true, None))
            }
            VoiceKeyAction::Commit(text) => {
                self.commit_text_with_record(ctx.frontend, &text);
                self.refresh_frontend_ui(ctx.frontend, ctx.inline_preedit);
                Some((true, Some(text)))
            }
            VoiceKeyAction::PassThrough => None,
        }
    }

    /// 消费核心会话产出的输入法动作并驱动前端与状态机同步
    ///
    /// # 设计原理
    /// - 保持对 `ImeOutputAction` 各变体的穷尽匹配，任何新增动作变体都会在此处
    ///   触发编译期检查，杜绝动作被静默丢弃。
    fn dispatch_ime_action<F: InputMethodFrontend>(
        &mut self,
        ctx: &mut ActionDispatchContext<'_, F>,
        action: ImeOutputAction,
    ) -> (bool, Option<String>) {
        match action {
            ImeOutputAction::PassThrough => (false, None),
            ImeOutputAction::Handled { update_ui } => self.dispatch_handled_action(ctx, update_ui),
            ImeOutputAction::ExpandGrid => self.dispatch_expand_grid_action(ctx),
            ImeOutputAction::Commit(text) => self.dispatch_commit_action(ctx, text, true),
            ImeOutputAction::CommitAndPassThrough(text) => {
                self.dispatch_commit_action(ctx, text, false)
            }
            ImeOutputAction::CommitPunctuation {
                text,
                cursor_offset,
            } => self.commit_punctuation_action(ctx, text, cursor_offset),
            ImeOutputAction::CommitPartial { committed, .. } => {
                self.dispatch_commit_action(ctx, committed, true)
            }
            ImeOutputAction::Reset => {
                self.reset_input_state_with_frontend(ctx);
                (true, None)
            }
            ImeOutputAction::UndoCommitAndRestore { delete_bytes } => {
                ctx.frontend.delete_surrounding_text(delete_bytes, 0);
                self.refresh_frontend_ui(ctx.frontend, ctx.inline_preedit);
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
                self.dispatch_mode_changed_action(ctx, new_mode, &committed);
                (was_consumed, committed)
            }
        }
    }

    /// 派发"仅刷新界面"类动作：按需刷新候选框并返回消费结果
    ///
    /// # 设计原理
    /// - `Handled` 与 `ExpandGrid` 的界面刷新行为一致，仅刷新时机存在差异，
    ///   故统一在此处集中实现刷新动作，避免两处逻辑各自漂移。
    fn dispatch_handled_action<F: InputMethodFrontend>(
        &mut self,
        ctx: &mut ActionDispatchContext<'_, F>,
        update_ui: bool,
    ) -> (bool, Option<String>) {
        if update_ui {
            self.refresh_frontend_ui(ctx.frontend, ctx.inline_preedit);
        }
        (true, None)
    }

    /// 派发候选框展开动作：刷新界面并返回消费结果
    fn dispatch_expand_grid_action<F: InputMethodFrontend>(
        &mut self,
        ctx: &mut ActionDispatchContext<'_, F>,
    ) -> (bool, Option<String>) {
        self.refresh_frontend_ui(ctx.frontend, ctx.inline_preedit);
        (true, None)
    }

    /// 派发文本上屏动作：提交文本、刷新界面并返回消费结果
    ///
    /// # 设计原理
    /// - `consumed` 区分普通上屏与"上屏并透传"两种语义，其余处理步骤完全一致。
    fn dispatch_commit_action<F: InputMethodFrontend>(
        &mut self,
        ctx: &mut ActionDispatchContext<'_, F>,
        text: String,
        consumed: bool,
    ) -> (bool, Option<String>) {
        self.commit_text_with_record(ctx.frontend, &text);
        self.refresh_frontend_ui(ctx.frontend, ctx.inline_preedit);
        (consumed, Some(text))
    }

    /// 派发成对标点上屏动作：提交文本、按需移动光标并刷新界面
    ///
    /// # 设计原理
    /// - 执行顺序固定为"提交文本 -> 移动光标 -> 刷新界面"，光标位移必须在文本提交之后生效，
    ///   否则成对符号的补全位置会错位。
    fn commit_punctuation_action<F: InputMethodFrontend>(
        &mut self,
        ctx: &mut ActionDispatchContext<'_, F>,
        text: String,
        cursor_offset: i32,
    ) -> (bool, Option<String>) {
        self.commit_text_with_record(ctx.frontend, &text);
        if cursor_offset != 0 {
            ctx.frontend.move_cursor(cursor_offset);
        }
        self.refresh_frontend_ui(ctx.frontend, ctx.inline_preedit);
        (true, Some(text))
    }

    /// 派发输入模式切换的状态同步：广播状态快照、同步前端输入模式并提交附带文本
    ///
    /// # 设计原理
    /// - 模式切换后必须先向共享标志容器广播最新状态快照，再同步前端输入模式，
    ///   保证托盘指示器与输入法前端在同一轮事件内保持一致。
    fn dispatch_mode_changed_action<F: InputMethodFrontend>(
        &mut self,
        ctx: &mut ActionDispatchContext<'_, F>,
        new_mode: InputMode,
        committed: &Option<String>,
    ) {
        self.window_tracker.update_active_mode(new_mode);
        self.broadcast_state_snapshot();
        ctx.frontend.sync_input_mode(new_mode);
        if let Some(text) = committed {
            ctx.frontend.commit_text(text);
        }
        self.refresh_frontend_ui(ctx.frontend, ctx.inline_preedit);
    }

    /// 重置核心输入状态并同步前端候选框与预编辑区
    ///
    /// # 设计原理
    /// - 状态重置必须同时清理内核会话状态与前端可见界面，避免残留候选框遮挡用户输入。
    fn reset_input_state_with_frontend<F: InputMethodFrontend>(
        &mut self,
        ctx: &mut ActionDispatchContext<'_, F>,
    ) {
        self.core.reset_input_state();
        ctx.frontend.hide_candidates();
        ctx.frontend.update_preedit("", 0);
    }

    /// 轮询语音后台事件流并在必要时通知前端更新或提交
    pub fn poll_voice_cycle<F: InputMethodFrontend>(
        &mut self,
        frontend: &mut F,
        inline_preedit: bool,
    ) -> Option<String> {
        if let Some(ref mut vm) = self.voice_manager
            && vm.is_active()
        {
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
        None
    }
}
