//! 输入法核心会话按键事件分发流水线与决策响应
//!
//! # 设计原理
//! - **实现初衷**：为所有前端协议（Wayland V1、Wayland V2、IBus）提供统一且唯一的按键决策流水线，
//!   消除不同协议实现中重复手写的密码模式规避、CapsLock 截断提交、中英模式切换与按键追踪逻辑。
//! - **核心优势**：单一状态真相源（Single Source of Truth）；实现跨平台按键处理行为 100% 对齐；
//!   任何按键处理的改进或修复仅需在此单一模块中维护即可全局生效。
//! - **代价与局限**：协议适配层需将平台特定事件参数包装为 [`KeyProcessContext`] 并消费 [`ImeOutputAction`]。

use waylan_core::input::*;
use waylan_core::{to_full_width_char, to_full_width_string, InputMode, TextAction};

use super::key_handler::{handle_composition_key_with_params, CompositionKeyParams};
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
            self.shift_sm.reset();
            if !ctx.is_press {
                let was_consumed = self.consumed_keys.take_release(ctx.keycode);
                return if was_consumed {
                    ImeOutputAction::Handled { update_ui: false }
                } else {
                    ImeOutputAction::PassThrough
                };
            }
            self.consumed_keys.record_press(ctx.keycode, false);
            return ImeOutputAction::PassThrough;
        }

        // 3. CapsLock 处理：按下时若有拼音则提交拼音，但物理按键一律透传给系统切换大写灯
        if ctx.keycode == KEY_CAPSLOCK {
            self.shift_sm.reset();
            if ctx.is_press {
                let (was_consumed, committed) = self.handle_capslock(true);
                if let Some(text) = committed {
                    return ImeOutputAction::CommitAndPassThrough(text);
                }
                return if was_consumed {
                    ImeOutputAction::Handled { update_ui: true }
                } else {
                    ImeOutputAction::PassThrough
                };
            } else {
                let (was_consumed, _) = self.handle_capslock(false);
                return if was_consumed {
                    ImeOutputAction::Handled { update_ui: false }
                } else {
                    ImeOutputAction::PassThrough
                };
            }
        }

        // 4. 按键释放事件处理
        if !ctx.is_press {
            if self.is_caps_locked() {
                self.shift_sm.reset();
                let was_consumed = self.consumed_keys.take_release(ctx.keycode);
                return if was_consumed {
                    ImeOutputAction::Handled { update_ui: false }
                } else {
                    ImeOutputAction::PassThrough
                };
            }

            let (was_consumed, toggled_mode, committed) = self.handle_key_release(ctx.keycode);
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
            return if was_consumed {
                ImeOutputAction::Handled { update_ui: true }
            } else {
                ImeOutputAction::PassThrough
            };
        }

        // 5. 按键按下阶段：调用 handle_key_press 进行核心算法与快捷键决策
        let result = self.handle_key_press(ctx.keycode, now, ctx.surrounding_prefix);
        let is_consumed = !matches!(result, ImeOutputAction::PassThrough);
        self.consumed_keys.record_press(ctx.keycode, is_consumed);
        result
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

        // 英文模式、CapsLock 锁定或按住 Ctrl/Alt 时一律原生透传
        if self.is_caps_locked() || self.has_ctrl_or_alt() {
            return ImeOutputAction::PassThrough;
        }

        // 若处于英文模式但开启了全角字符，直通字母转换为全角字符输出
        if self.mode == InputMode::English {
            if self.full_width_char && !self.is_shift_active() {
                if let Some(ch) = self.keymap_layout.get_char(keycode) {
                    let full = to_full_width_char(ch);
                    return ImeOutputAction::Commit(full.to_string());
                }
            }
            return ImeOutputAction::PassThrough;
        }

        // 1. 若处于拼音组词状态，优先流转大网格展开态或拼音状态机
        if !self.engine.raw_input().is_empty() {
            if self.is_grid_expanded {
                if let Some(action) = self.handle_grid_expanded_key(keycode, now) {
                    return action;
                }
            }

            let params = CompositionKeyParams {
                key: keycode,
                shift_active: self.is_shift_active(),
                page_keys: self.page_keys,
                now,
            };
            if let Some(action) = handle_composition_key_with_params(
                &mut self.engine,
                &mut self.punctuation_processor,
                params,
            ) {
                if action == ImeOutputAction::ExpandGrid {
                    self.expand_grid();
                }
                return match action {
                    ImeOutputAction::Commit(text) => {
                        let final_text = if self.full_width_char {
                            to_full_width_string(&text)
                        } else {
                            text
                        };
                        ImeOutputAction::Commit(final_text)
                    }
                    ImeOutputAction::CommitPartial {
                        committed,
                        remaining_preedit,
                    } => {
                        let final_committed = if self.full_width_char {
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
                };
            }
        }

        // 2. 若输入缓冲区为空（空闲输入），拦截并处理智能标点
        if self.engine.raw_input().is_empty() {
            // 空闲下的退格与空格保持透传
            if keycode == KEY_BACKSPACE || keycode == KEY_SPACE {
                return ImeOutputAction::PassThrough;
            }

            // 检查标点符号映射
            if let Some(punct_char) =
                keycode_to_punctuation_with_shift(keycode, self.is_shift_active())
            {
                let actions_opt = match context_surrounding {
                    Some(ctx) => self
                        .punctuation_processor
                        .process_punctuation_with_context(punct_char, ctx),
                    None => self.punctuation_processor.process_punctuation(punct_char),
                };
                if let Some(actions) = actions_opt {
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
                    if !commit_str.is_empty() {
                        let final_commit = if self.full_width_char {
                            to_full_width_string(&commit_str)
                        } else {
                            commit_str
                        };
                        if cursor_offset != 0 {
                            return ImeOutputAction::CommitPunctuation {
                                text: final_commit,
                                cursor_offset,
                            };
                        } else {
                            return ImeOutputAction::Commit(final_commit);
                        }
                    }
                }
            }

            // 空闲下的数字按键记录上下文（用于小数点等数字语境智能半角判定）
            if !self.is_shift_active() {
                if let Some(digit_ch) = keycode_to_ascii_digit_char(keycode) {
                    self.punctuation_processor
                        .record_commit(&digit_ch.to_string());
                    if self.full_width_char {
                        let full = to_full_width_char(digit_ch);
                        return ImeOutputAction::Commit(full.to_string());
                    }
                    return ImeOutputAction::PassThrough;
                }
            }
        }

        // 3. 拼音字母键录入
        if let Some(ch) = self.keymap_layout.get_char(keycode) {
            self.engine.push_char(ch);
            return ImeOutputAction::Handled { update_ui: true };
        }

        ImeOutputAction::PassThrough
    }
}
