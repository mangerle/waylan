//! 输入法核心会话按键事件分发流水线与决策响应
//!
//! # 设计原理
//! - **实现初衷**：为所有前端协议（Wayland V1、Wayland V2、IBus）提供统一且唯一的按键决策流水线，
//!   消除不同协议实现中重复手写的密码模式规避、CapsLock 截断提交、中英模式切换与按键追踪逻辑。
//! - **核心优势**：单一状态真相源（Single Source of Truth）；实现跨平台按键处理行为 100% 对齐；
//!   任何按键处理的改进或修复仅需在此单一模块中维护即可全局生效。
//! - **代价与局限**：协议适配层需将平台特定事件参数包装为 [`KeyProcessContext`] 并消费 [`ImeOutputAction`]。

use log::debug;
use waylan_core::input::*;
use waylan_core::{InputMode, TextAction, to_full_width_char, to_full_width_string};

use super::key_handler::{CompositionKeyParams, handle_composition_key_with_params};
use super::{ImeCoreSession, ImeOutputAction};

/// 核心输入法按键处理上下文
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyProcessContext<'a> {
    /// Linux evdev 物理键码
    pub keycode: u32,
    /// 按键动作：true 为按下 (press)，false 为释放 (release)
    pub is_press: bool,
    /// 是否处于密码框、PIN 码或系统锁屏等免拦截安全上下文
    pub is_password_or_locked: bool,
    /// 光标前周围文本切片（用于中文智能标点成对配对与数字语境半角判定）
    pub surrounding_prefix: Option<&'a str>,
}

impl ImeCoreSession {
    /// 核心会话按键事件统一决策分发流水线
    ///
    /// # 设计原理
    /// - **单一状态真相源**：将密码模式规避、CapsLock 截断提交、修饰键跟踪、
    ///   按键释放追踪、中英 Shift 切换、全局模式快捷键与核心拼音/标点组词统一闭环。
    /// - **跨端零差异**：Wayland V1、Wayland V2 与 IBus 前端无需重复编写分发分支，
    ///   只需将物理按键封装为 [`KeyProcessContext`] 传入，并执行返回的 [`ImeOutputAction`]。
    pub fn process_key(&mut self, ctx: KeyProcessContext<'_>, now: u64) -> ImeOutputAction {
        // 1. 更新修饰键掩码与防误触状态机
        self.update_modifier_by_key(ctx.keycode, ctx.is_press);

        // 2. 密码框、PIN 码或系统锁屏安全保护：完全规避输入法算法，全量直通
        if ctx.is_password_or_locked {
            return self.handle_protected_context_key(ctx.keycode, ctx.is_press);
        }

        // 3. CapsLock 处理：按下时若有拼音则提交拼音，但物理按键一律透传给系统切换大写灯
        if ctx.keycode == KEY_CAPSLOCK {
            return self.handle_capslock_key(ctx.is_press);
        }

        // 4. 按键释放事件处理
        if !ctx.is_press {
            return self.handle_key_release_event(ctx.keycode);
        }

        // 5. 按键按下阶段：调用 handle_key_press 进行核心算法与快捷键决策
        let result = self.handle_key_press(ctx.keycode, now, ctx.surrounding_prefix);
        let is_consumed = !matches!(result, ImeOutputAction::PassThrough);
        self.consumed_keys.record_press(ctx.keycode, is_consumed);
        result
    }

    /// 密码框、PIN 码或系统锁屏上下文下的按键直通处理
    fn handle_protected_context_key(&mut self, keycode: u32, is_press: bool) -> ImeOutputAction {
        self.shift_sm.reset();
        if is_press {
            debug!(
                "[会话调度] 处于受保护上下文（密码框或锁屏直通），透传物理按键: 键码={}",
                keycode
            );
        }
        if !is_press {
            let was_consumed = self.consumed_keys.take_release(keycode);
            return if was_consumed {
                ImeOutputAction::Handled { update_ui: false }
            } else {
                ImeOutputAction::PassThrough
            };
        }
        self.consumed_keys.record_press(keycode, false);
        ImeOutputAction::PassThrough
    }

    /// CapsLock 按键处理（按下时若有拼音则提交拼音，翻转大写锁定状态，物理按键一律透传给系统切换大写灯）
    fn handle_capslock_key(&mut self, is_press: bool) -> ImeOutputAction {
        self.shift_sm.reset();
        if is_press {
            let (was_consumed, committed) = self.handle_capslock(true);
            let next_locked = !self.is_caps_locked();
            self.set_caps_lock(next_locked);
            if self.input_mode() != InputMode::English {
                let effective_mode = if next_locked {
                    InputMode::English
                } else {
                    self.input_mode()
                };
                return ImeOutputAction::ModeChanged {
                    new_mode: effective_mode,
                    committed,
                    was_consumed,
                };
            } else {
                if let Some(text) = committed {
                    return ImeOutputAction::CommitAndPassThrough(text);
                }
                return if was_consumed {
                    ImeOutputAction::Handled { update_ui: false }
                } else {
                    ImeOutputAction::PassThrough
                };
            }
        }
        let (was_consumed, _) = self.handle_capslock(false);
        if was_consumed {
            ImeOutputAction::Handled { update_ui: false }
        } else {
            ImeOutputAction::PassThrough
        }
    }

    /// 按键释放事件处理（大写锁定态严格保持物理锁定，屏蔽 Shift 模式切换）
    fn handle_key_release_event(&mut self, keycode: u32) -> ImeOutputAction {
        // 大写锁定态属于最高优先级大写英文输入状态：单按 Shift 绝不改变大写锁定，亦不切回中文
        if self.is_caps_locked() {
            self.shift_sm.reset();
            let was_consumed = self.consumed_keys.take_release(keycode);
            return if was_consumed {
                ImeOutputAction::Handled { update_ui: false }
            } else {
                ImeOutputAction::PassThrough
            };
        }

        let (was_consumed, toggled_mode, committed) = self.handle_key_release(keycode);
        if let Some(new_mode) = toggled_mode {
            return ImeOutputAction::ModeChanged {
                new_mode,
                committed,
                was_consumed,
            };
        }
        if let Some(text) = committed {
            return ImeOutputAction::Commit(text);
        }
        if was_consumed {
            ImeOutputAction::Handled { update_ui: true }
        } else {
            ImeOutputAction::PassThrough
        }
    }

    /// 处理核心按键按下事件，返回会话决策 [`ImeOutputAction`]
    pub fn handle_key_press(
        &mut self,
        keycode: u32,
        now: u64,
        context_surrounding: Option<&str>,
    ) -> ImeOutputAction {
        // 0. 全局模式与功能快捷键优先捕获
        if let Some(toggle) = self.handle_global_shortcuts(keycode) {
            return ImeOutputAction::ShortcutToggled(toggle);
        }

        // 英文模式、CapsLock 锁定或按住 Ctrl/Alt 时的直通或全角处理
        if let Some(action) = self.dispatch_passthrough_or_fullwidth_english(keycode) {
            return action;
        }

        // 1. 若处于拼音组词状态，流转大网格展开态或拼音状态机
        if !self.engine.raw_input().is_empty()
            && let Some(action) = self.dispatch_composing_key(keycode, now)
        {
            return action;
        }

        // 2. 若输入缓冲区为空（空闲输入），优先处理“选错即撤回”、代码/URL/路径自动直通英文，
        //    再拦截并处理智能标点与数字上下文
        if self.engine.raw_input().is_empty() {
            if let Some(action) = self.try_undo_last_commit(keycode, now) {
                return action;
            }
            self.last_commit_undo = None;
            if keycode == KEY_BACKSPACE || keycode == KEY_SPACE {
                return ImeOutputAction::PassThrough;
            }
            if context_surrounding.is_some_and(should_auto_passthrough_english)
                && (self.keymap_layout.get_char(keycode).is_some()
                    || keycode_to_punctuation_with_shift(keycode, self.is_shift_active()).is_some())
            {
                return ImeOutputAction::PassThrough;
            }
            if let Some(action) = self.dispatch_idle_punctuation(keycode, context_surrounding) {
                return action;
            }
            if let Some(action) = self.dispatch_idle_digit(keycode) {
                return action;
            }
        }

        // 3. 拼音字母键录入
        if let Some(ch) = self.keymap_layout.get_char(keycode) {
            self.engine.push_char(ch);
            return ImeOutputAction::Handled { update_ui: true };
        }

        ImeOutputAction::PassThrough
    }

    /// 处理英文模式、CapsLock 锁定及控制键按下时的透传或大写/全角字母转换
    fn dispatch_passthrough_or_fullwidth_english(&self, keycode: u32) -> Option<ImeOutputAction> {
        // 0. 组词态下的 Ctrl+Delete（忘词）与空闲态紧随上屏后的 Ctrl+Backspace（选错撤回）放行，
        //    其余控制组合键一律透传
        let is_forget_shortcut = self.mode == InputMode::Chinese
            && !self.engine.raw_input().is_empty()
            && keycode == KEY_DELETE
            && self.is_ctrl_active()
            && !self.is_alt_active();
        let is_undo_commit_shortcut = self.mode == InputMode::Chinese
            && self.engine.raw_input().is_empty()
            && self.last_commit_undo.is_some()
            && keycode == KEY_BACKSPACE
            && self.is_ctrl_active()
            && !self.is_alt_active();
        if self.has_ctrl_or_alt() && !is_forget_shortcut && !is_undo_commit_shortcut {
            return Some(ImeOutputAction::PassThrough);
        }

        // 2. CapsLock 锁定状态下的特殊定制
        if self.is_caps_locked() {
            // 2.1 大写锁定下单按 Shift 键彻底静默吞掉（不透传给系统，无任何系统事件）
            if keycode == KEY_LEFTSHIFT || keycode == KEY_RIGHTSHIFT {
                return Some(ImeOutputAction::Handled { update_ui: false });
            }

            // 2.2 字母键处理：无论按不按 Shift，字母都必须始终保持大写，绝不允许因为按了 Shift 输出小写
            if let Some(ch) = self.keymap_layout.get_char(keycode)
                && ch.is_ascii_alphabetic()
            {
                let upper = ch.to_ascii_uppercase();
                // 当按住 Shift 时，底层系统/XKB 默认会将 CapsLock+Shift 反转为小写字母；
                // 此时直接由输入法拦截并提交大写字符，彻底杜绝输出小写
                if self.is_full_width_char() {
                    return Some(ImeOutputAction::Commit(
                        to_full_width_char(upper).to_string(),
                    ));
                }
                if self.is_shift_active() {
                    return Some(ImeOutputAction::Commit(upper.to_string()));
                }
                return Some(ImeOutputAction::PassThrough);
            }

            // 2.3 其他非字母键（数字、标点符号如 Shift+1->!、回车、退格等）透传
            return Some(ImeOutputAction::PassThrough);
        }

        // 3. 常规英文模式处理
        if self.mode == InputMode::English {
            if self.is_full_width_char()
                && !self.is_shift_active()
                && let Some(ch) = self.keymap_layout.get_char(keycode)
            {
                let full = to_full_width_char(ch);
                return Some(ImeOutputAction::Commit(full.to_string()));
            }
            return Some(ImeOutputAction::PassThrough);
        }

        None
    }

    /// 组词态按键分流（网格展开键、翻页、选词、字母编辑）
    fn dispatch_composing_key(&mut self, keycode: u32, now: u64) -> Option<ImeOutputAction> {
        if self.is_grid_expanded()
            && let Some(action) = self.handle_grid_expanded_key(keycode, now)
        {
            return Some(action);
        }

        // 将提交前的原始拼音备份到栈上（超长时回退堆分配），避免普通击键、翻页等非提交路径产生堆分配
        let raw_slice = self.engine.raw_input();
        let mut stack_raw = [0u8; 128];
        let mut heap_raw: Option<String> = None;
        let stack_len = if raw_slice.len() <= stack_raw.len() {
            stack_raw[..raw_slice.len()].copy_from_slice(raw_slice.as_bytes());
            Some(raw_slice.len())
        } else {
            heap_raw = Some(raw_slice.to_string());
            None
        };
        let params = CompositionKeyParams {
            key: keycode,
            shift_active: self.is_shift_active(),
            ctrl_active: self.is_ctrl_active(),
            page_keys: self.page_keys,
            now,
        };
        let action = handle_composition_key_with_params(self, params)?;

        if action == ImeOutputAction::ExpandGrid {
            self.expand_grid();
        }

        Some(match action {
            ImeOutputAction::Commit(text) => {
                self.collapse_card();
                let final_text = if self.is_full_width_char() {
                    to_full_width_string(&text)
                } else {
                    text
                };
                let raw_before_commit = match (stack_len, heap_raw) {
                    (Some(len), _) => std::str::from_utf8(&stack_raw[..len])
                        .unwrap_or("")
                        .to_string(),
                    (None, Some(s)) => s,
                    (None, None) => String::new(),
                };
                if !raw_before_commit.is_empty() && !final_text.is_empty() {
                    self.last_commit_undo = Some(super::LastCommitUndoState {
                        raw_pinyin: raw_before_commit,
                        committed_text: final_text.clone(),
                        committed_at: now,
                    });
                }
                ImeOutputAction::Commit(final_text)
            }
            ImeOutputAction::CommitPartial {
                committed,
                remaining_preedit,
            } => {
                let final_committed = if self.is_full_width_char() {
                    to_full_width_string(&committed)
                } else {
                    committed
                };
                ImeOutputAction::CommitPartial {
                    committed: final_committed,
                    remaining_preedit,
                }
            }
            other => other,
        })
    }

    /// 空闲输入状态下处理智能标点
    fn dispatch_idle_punctuation(
        &mut self,
        keycode: u32,
        context_surrounding: Option<&str>,
    ) -> Option<ImeOutputAction> {
        let punct_char = keycode_to_punctuation_with_shift(keycode, self.is_shift_active())?;
        let actions = match context_surrounding {
            Some(ctx) => self
                .punctuation_processor
                .process_punctuation_with_context(punct_char, ctx)?,
            None => self.punctuation_processor.process_punctuation(punct_char)?,
        };

        let mut commit_str = String::new();
        let mut cursor_offset = 0;
        for act in actions {
            match act {
                TextAction::Commit(t) => {
                    self.punctuation_processor.record_commit(&t);
                    commit_str.push_str(&t);
                }
                TextAction::MoveCursor(off) => {
                    cursor_offset += off;
                }
                _ => {}
            }
        }

        if commit_str.is_empty() {
            return None;
        }

        let final_commit = if self.is_full_width_char() {
            to_full_width_string(&commit_str)
        } else {
            commit_str
        };

        Some(if cursor_offset != 0 {
            ImeOutputAction::CommitPunctuation {
                text: final_commit,
                cursor_offset,
            }
        } else {
            ImeOutputAction::Commit(final_commit)
        })
    }

    /// 空闲状态下记录数字上下文以支持小数点智能判定
    fn dispatch_idle_digit(&mut self, keycode: u32) -> Option<ImeOutputAction> {
        if self.is_shift_active() {
            return None;
        }
        let digit_ch = keycode_to_ascii_digit_char(keycode)?;
        let mut digit_buf = [0u8; 4];
        self.punctuation_processor
            .record_commit(digit_ch.encode_utf8(&mut digit_buf));
        if self.is_full_width_char() {
            let full = to_full_width_char(digit_ch);
            return Some(ImeOutputAction::Commit(full.to_string()));
        }
        Some(ImeOutputAction::PassThrough)
    }

    /// 选错即撤回：紧随拼音选词上屏后按下 `Ctrl+Backspace`（15 秒内），删除刚上屏文本并恢复原拼音编辑态
    fn try_undo_last_commit(&mut self, keycode: u32, now: u64) -> Option<ImeOutputAction> {
        if keycode != KEY_BACKSPACE || !self.is_ctrl_active() || self.is_alt_active() {
            return None;
        }
        let undo = self.last_commit_undo.take()?;
        if now.saturating_sub(undo.committed_at) > 15 {
            return None;
        }
        let delete_bytes = undo.committed_text.len() as u32;
        if delete_bytes == 0 || undo.raw_pinyin.is_empty() {
            return None;
        }
        self.reset_viewport();
        self.engine.reset();
        for ch in undo.raw_pinyin.chars() {
            self.engine.push_char(ch);
        }
        Some(ImeOutputAction::UndoCommitAndRestore { delete_bytes })
    }
}

pub use super::auto_passthrough::should_auto_passthrough_english;

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod passthrough_tests;
