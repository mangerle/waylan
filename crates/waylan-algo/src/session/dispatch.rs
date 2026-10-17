//! 输入法核心会话按键事件分发流水线与决策响应
//!
//! # 设计原理
//! - **实现初衷**：为所有前端协议（Wayland V1、Wayland V2、IBus）提供统一且唯一的按键决策流水线，
//!   消除不同协议实现中重复手写的密码模式规避、CapsLock 截断提交、中英模式切换与按键追踪逻辑。
//! - **核心优势**：单一状态真相源（Single Source of Truth）；实现跨平台按键处理行为 100% 对齐；
//!   任何按键处理的改进或修复仅需在此单一模块中维护即可全局生效。
//! - **代价与局限**：协议适配层需将平台特定事件参数包装为 [`KeyProcessContext`] 并消费 [`KeyProcessResult`]。

use crate::composition::{
    handle_composition_key_with_params, CompositionKeyParams, KeyProcessAction,
};
use crate::session::{ImeAction, ImeCoreSession, ShortcutToggleAction};
use waylan_core::input::*;
use waylan_core::{to_full_width_char, to_full_width_string, InputMode, TextAction};

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

/// 核心会话对按键事件的决策与状态响应
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyProcessResult {
    /// 原生透传：按键未被输入法拦截，协议层需将硬件按键原生发送给客户端/应用程序
    PassThrough,
    /// 内部已消费并处理：协议层需消费该按键，update_ui 指示是否需要刷新预编辑与候选框
    Handled { update_ui: bool },
    /// 请求展开大候选词网格卡片
    ExpandGrid,
    /// 触发纯文本直接上屏提交（如空格选词、数字选词等，按键已被消费）
    Commit(String),
    /// CapsLock 截断提交：有未上屏拼音需提交，但按键本身不消费，需透传给系统切换大小写
    CommitAndPassThrough(String),
    /// 成对标点符号上屏并附带光标相对位移（如引号/括号自动配对后光标向左移动若干字符）
    CommitPunctuation { text: String, cursor_offset: i32 },
    /// 分段选词部分上屏，包含本次已提交文本与剩余预编辑拼音
    CommitPartial {
        committed: String,
        remaining_preedit: String,
    },
    /// 取消/重置当前输入（如 Escape 取消）
    Reset,
    /// 模式或功能快捷键切换（简繁切换、字符全半角、标点全半角）
    ShortcutToggled(ShortcutToggleAction),
    /// 输入模式发生切换（如按 Shift 切换中英文）
    ModeChanged {
        new_mode: InputMode,
        committed: Option<String>,
        was_consumed: bool,
    },
}

impl ImeCoreSession {
    /// 核心会话按键事件统一决策分发流水线
    ///
    /// # 设计原理
    /// - **单一状态真相源**：将密码模式规避、CapsLock 截断提交、修饰键跟踪、
    ///   按键释放追踪、中英 Shift 切换、全局模式快捷键与核心拼音/标点组词统一闭环。
    /// - **跨端零差异**：Wayland V1、Wayland V2 与 IBus 前端无需重复编写分发分支，
    ///   只需将物理按键封装为 [`KeyProcessContext`] 传入，并执行返回的 [`KeyProcessResult`]。
    pub fn process_key(&mut self, ctx: KeyProcessContext<'_>, now: u64) -> KeyProcessResult {
        // 1. 更新修饰键掩码与防误触状态机
        self.update_modifier_by_key(ctx.keycode, ctx.is_press);

        // 2. 密码框、PIN 码或系统锁屏安全保护：完全规避输入法算法，全量直通
        if ctx.is_password_or_locked {
            self.shift_sm.reset();
            if !ctx.is_press {
                let was_consumed = self.consumed_keys.take_release(ctx.keycode);
                return if was_consumed {
                    KeyProcessResult::Handled { update_ui: false }
                } else {
                    KeyProcessResult::PassThrough
                };
            }
            self.consumed_keys.record_press(ctx.keycode, false);
            return KeyProcessResult::PassThrough;
        }

        // 3. CapsLock 处理：按下时若有拼音则提交拼音，但物理按键一律透传给系统切换大写灯
        if ctx.keycode == KEY_CAPSLOCK {
            self.shift_sm.reset();
            if ctx.is_press {
                let (was_consumed, committed) = self.handle_capslock(true);
                if let Some(text) = committed {
                    return KeyProcessResult::CommitAndPassThrough(text);
                }
                return if was_consumed {
                    KeyProcessResult::Handled { update_ui: true }
                } else {
                    KeyProcessResult::PassThrough
                };
            } else {
                let (was_consumed, _) = self.handle_capslock(false);
                return if was_consumed {
                    KeyProcessResult::Handled { update_ui: false }
                } else {
                    KeyProcessResult::PassThrough
                };
            }
        }

        // 4. 按键释放事件处理
        if !ctx.is_press {
            if self.is_caps_locked() {
                self.shift_sm.reset();
                let was_consumed = self.consumed_keys.take_release(ctx.keycode);
                return if was_consumed {
                    KeyProcessResult::Handled { update_ui: false }
                } else {
                    KeyProcessResult::PassThrough
                };
            }

            let (was_consumed, toggled_mode, committed) = self.handle_key_release(ctx.keycode);
            if let Some(new_mode) = toggled_mode {
                return KeyProcessResult::ModeChanged {
                    new_mode,
                    committed,
                    was_consumed,
                };
            }
            if let Some(text) = committed {
                return KeyProcessResult::Commit(text);
            }
            return if was_consumed {
                KeyProcessResult::Handled { update_ui: true }
            } else {
                KeyProcessResult::PassThrough
            };
        }

        // 5. 按键按下阶段：调用 handle_key_press 进行核心算法与快捷键决策
        let action = self.handle_key_press(ctx.keycode, now, ctx.surrounding_prefix);
        let result = match action {
            ImeAction::PassThrough => KeyProcessResult::PassThrough,
            ImeAction::Handled => KeyProcessResult::Handled { update_ui: true },
            ImeAction::ExpandGrid => KeyProcessResult::ExpandGrid,
            ImeAction::Commit(text) => KeyProcessResult::Commit(text),
            ImeAction::CommitPunctuation {
                text,
                cursor_offset,
            } => KeyProcessResult::CommitPunctuation {
                text,
                cursor_offset,
            },
            ImeAction::CommitPartial {
                committed,
                remaining_preedit,
            } => KeyProcessResult::CommitPartial {
                committed,
                remaining_preedit,
            },
            ImeAction::Reset => KeyProcessResult::Reset,
            ImeAction::ShortcutToggled(toggle) => KeyProcessResult::ShortcutToggled(toggle),
        };

        let is_consumed = !matches!(result, KeyProcessResult::PassThrough);
        self.consumed_keys.record_press(ctx.keycode, is_consumed);
        result
    }

    /// 处理核心按键按下事件，返回会话决策 [`ImeAction`]
    pub fn handle_key_press(
        &mut self,
        keycode: u32,
        now: u64,
        context_surrounding: Option<&str>,
    ) -> ImeAction {
        // 0. 全局模式与功能快捷键优先捕获
        if let Some(toggle) = self.handle_global_shortcuts(keycode) {
            return ImeAction::ShortcutToggled(toggle);
        }

        // 英文模式、CapsLock 锁定或按住 Ctrl/Alt 时一律原生透传
        if self.is_caps_locked() || self.has_ctrl_or_alt() {
            return ImeAction::PassThrough;
        }

        // 若处于英文模式但开启了全角字符，直通字母转换为全角字符输出
        if self.mode == InputMode::English {
            if self.full_width_char && !self.is_shift_active() {
                if let Some(ch) = self.keymap_layout.get_char(keycode) {
                    let full = to_full_width_char(ch);
                    return ImeAction::Commit(full.to_string());
                }
            }
            return ImeAction::PassThrough;
        }

        // 1. 若处于拼音组词状态，优先流转拼音状态机
        if !self.engine.raw_input().is_empty() {
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
                return match action {
                    KeyProcessAction::PassThrough => ImeAction::PassThrough,
                    KeyProcessAction::Handled => ImeAction::Handled,
                    KeyProcessAction::ExpandGrid => ImeAction::ExpandGrid,
                    KeyProcessAction::Commit(text) => {
                        let final_text = if self.full_width_char {
                            to_full_width_string(&text)
                        } else {
                            text
                        };
                        ImeAction::Commit(final_text)
                    }
                    KeyProcessAction::CommitPartial {
                        committed,
                        remaining_preedit,
                    } => {
                        let final_committed = if self.full_width_char {
                            to_full_width_string(&committed)
                        } else {
                            committed
                        };
                        ImeAction::CommitPartial {
                            committed: final_committed,
                            remaining_preedit,
                        }
                    }
                    KeyProcessAction::Reset => ImeAction::Reset,
                };
            }
        }

        // 2. 若输入缓冲区为空（空闲输入），拦截并处理智能标点
        if self.engine.raw_input().is_empty() {
            // 空闲下的退格与空格保持透传
            if keycode == KEY_BACKSPACE || keycode == KEY_SPACE {
                return ImeAction::PassThrough;
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
                            return ImeAction::CommitPunctuation {
                                text: final_commit,
                                cursor_offset,
                            };
                        } else {
                            return ImeAction::Commit(final_commit);
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
                        return ImeAction::Commit(full.to_string());
                    }
                    return ImeAction::PassThrough;
                }
            }
        }

        // 3. 拼音字母键录入
        if let Some(ch) = self.keymap_layout.get_char(keycode) {
            self.engine.push_char(ch);
            return ImeAction::Handled;
        }

        ImeAction::PassThrough
    }
}
