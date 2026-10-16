//! 输入法核心通用会话管理与按键驱动状态机
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1、Wayland V2 与 IBus 等多协议端在按键分流、
//!   CapsLock 截断提交、中英 Shift 模式切换与智能成对标点处理上的样板代码重复。
//! - **核心优势**：纯逻辑抽象，零特定协议依赖；单一状态真实源（Single Source of Truth）；
//!   多协议端共享相同业务输入法决策，彻底规避平台行为不一致与维护失步。
//! - **代价与局限**：协议适配层需消费本模块产出的 [`ImeAction`] 动作并映射为各协议具体调用，
//!   增加了单层轻量结果解构，但彻底隔绝了平台底层对复杂打字业务逻辑的污染。

use crate::composition::{
    handle_composition_key_with_params, CompositionKeyParams, KeyProcessAction,
};
use crate::engine::AlgoEngine;
use crate::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_core::input::*;
use waylan_core::{InputMode, TextAction};

/// 核心会话按键处理决策输出
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeAction {
    /// 按键未被输入法内部消费，协议层需将硬件按键原生透传（PassThrough）给宿主/客户端
    PassThrough,
    /// 按键已被消费，仅需刷新预编辑文本区与候选词 UI
    Handled,
    /// 触发文本直接上屏提交（如空格选词、数字选词、标点输入、CapsLock 截断提交等）
    Commit(String),
    /// 成对标点符号上屏并附带光标相对位移（如引号/括号自动配对后光标向左移动 1 格）
    CommitPunctuation { text: String, cursor_offset: i32 },
    /// 分段选词部分上屏，包含本次已提交文本与剩余预编辑拼音
    CommitPartial {
        committed: String,
        remaining_preedit: String,
    },
    /// 取消/重置当前输入（如 Escape 取消）
    Reset,
    /// 请求展开大候选词网格卡片（按下下方向键或自定义展开键）
    ExpandGrid,
}

/// 微澜输入法核心会话上下文与按键驱动状态机 (IME Core Session)
#[derive(Debug)]
pub struct ImeCoreSession {
    /// 底层算法引擎门面
    pub engine: AlgoEngine,
    /// Shift 快速点按中英切换状态机
    pub shift_sm: ShiftToggleStateMachine,
    /// 中文智能标点与成对符号处理器
    pub punctuation_processor: PunctuationProcessor,
    /// 翻页与选词热键配置
    pub page_keys: PageKeyConfig,
    /// 动态键盘物理扫描码到字符的映射表（适配 QWERTY、Dvorak 等）
    pub keymap_layout: KeymapLayout,
    /// 当前输入模式（中文 / 英文 / V模式）
    pub mode: InputMode,
    /// 已被输入法消费的物理按键追踪器（保证按下与释放事件严格对称）
    pub consumed_keys: ConsumedKeyTracker,
    /// 统一修饰键状态跟踪器
    pub modifiers: ModifierState,
}

impl Default for ImeCoreSession {
    fn default() -> Self {
        Self::new(AlgoEngine::with_sample_data(
            crate::types::AlgoConfig::default(),
        ))
    }
}

impl ImeCoreSession {
    /// 创建带有指定算法引擎的核心会话
    pub fn new(engine: AlgoEngine) -> Self {
        Self {
            engine,
            shift_sm: ShiftToggleStateMachine::new(),
            punctuation_processor: PunctuationProcessor::new(),
            page_keys: PageKeyConfig::default(),
            keymap_layout: KeymapLayout::default(),
            mode: InputMode::Chinese,
            consumed_keys: ConsumedKeyTracker::new(),
            modifiers: ModifierState::default(),
        }
    }

    /// 获取当前按下的修饰键位掩码
    #[inline]
    pub fn mods_depressed(&self) -> u32 {
        self.modifiers.depressed
    }

    /// 获取当前锁定的修饰键位掩码
    #[inline]
    pub fn mods_locked(&self) -> u32 {
        self.modifiers.locked
    }

    /// 显式同步修饰键掩码
    #[inline]
    pub fn set_modifiers(&mut self, depressed: u32, locked: u32) {
        self.modifiers.depressed = depressed;
        self.modifiers.locked = locked;
    }

    /// 重置输入状态（清空修饰键、消费追踪、Shift 状态机、标点处理器与算法引擎）
    pub fn reset_input_state(&mut self) {
        self.modifiers.reset();
        self.consumed_keys.clear();
        self.shift_sm.reset();
        self.punctuation_processor.reset();
        self.engine.reset();
    }

    /// 查询当前是否处于 Shift 激活状态
    #[inline]
    pub fn is_shift_active(&self) -> bool {
        self.modifiers.is_shift_active()
    }

    /// 查询当前是否处于 CapsLock 大写锁定状态
    #[inline]
    pub fn is_caps_locked(&self) -> bool {
        self.modifiers.is_caps_locked()
    }

    /// 查询当前是否按下了 Ctrl、Alt 或 Super 修饰键
    #[inline]
    pub fn has_ctrl_or_alt(&self) -> bool {
        self.modifiers.has_ctrl_or_alt()
    }

    /// 根据按下的物理按键更新修饰键掩码与防误触状态机
    pub fn update_modifier_by_key(&mut self, keycode: u32, is_press: bool) {
        self.modifiers.update(keycode, is_press);
        if is_press {
            self.shift_sm.on_key_event(keycode, true, self.mode);
        }
    }

    /// 处理 CapsLock 按键事件
    ///
    /// 若处于按下阶段且当前输入引擎存在未上屏的拼音文本，则强制将拼音原文提取提交并重置引擎。
    pub fn handle_capslock(&mut self, is_press: bool) -> (bool, Option<String>) {
        if is_press {
            if !self.engine.raw_input().is_empty() {
                let raw = self.engine.raw_input().to_string();
                self.engine.reset();
                self.consumed_keys.record_press(KEY_CAPSLOCK, false);
                (false, Some(raw))
            } else {
                self.consumed_keys.record_press(KEY_CAPSLOCK, false);
                (false, None)
            }
        } else {
            (self.consumed_keys.take_release(KEY_CAPSLOCK), None)
        }
    }

    /// 处理按键释放阶段的模式切换与状态追踪
    ///
    /// 若触发了中英模式切换且切换为英文模式，当引擎有未上屏拼音时，将其原文提交并重置引擎。
    pub fn handle_key_release(
        &mut self,
        keycode: u32,
    ) -> (bool, Option<InputMode>, Option<String>) {
        let was_consumed = self.consumed_keys.take_release(keycode);
        let toggled_mode = self.shift_sm.on_key_event(keycode, false, self.mode);
        if let Some(new_mode) = toggled_mode {
            self.mode = new_mode;
            if new_mode == InputMode::English && !self.engine.raw_input().is_empty() {
                let raw = self.engine.raw_input().to_string();
                self.engine.reset();
                return (was_consumed, Some(new_mode), Some(raw));
            }
            return (was_consumed, Some(new_mode), None);
        }
        (was_consumed, None, None)
    }

    /// 处理核心按键按下事件，返回统一会话决策 ImeAction
    pub fn handle_key_press(
        &mut self,
        keycode: u32,
        now: u64,
        context_surrounding: Option<&str>,
    ) -> ImeAction {
        // 英文模式、CapsLock 锁定或按住 Ctrl/Alt 时一律原生透传
        if self.mode == InputMode::English || self.is_caps_locked() || self.has_ctrl_or_alt() {
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
                    KeyProcessAction::Commit(text) => ImeAction::Commit(text),
                    KeyProcessAction::CommitPartial {
                        committed,
                        remaining_preedit,
                    } => ImeAction::CommitPartial {
                        committed,
                        remaining_preedit,
                    },
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
                        if cursor_offset != 0 {
                            return ImeAction::CommitPunctuation {
                                text: commit_str,
                                cursor_offset,
                            };
                        } else {
                            return ImeAction::Commit(commit_str);
                        }
                    }
                }
            }

            // 空闲下的数字按键记录上下文（用于小数点等数字语境智能半角判定）
            if !self.is_shift_active() {
                if let Some(digit_ch) = keycode_to_ascii_digit_char(keycode) {
                    self.punctuation_processor
                        .record_commit(&digit_ch.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::LexiconTable;
    use crate::types::AlgoConfig;

    fn make_test_session() -> ImeCoreSession {
        let engine = AlgoEngine::new(LexiconTable::with_sample_data(), AlgoConfig::default());
        ImeCoreSession::new(engine)
    }

    #[test]
    fn test_ime_core_session_input_and_commit() {
        let mut session = make_test_session();
        // 键入 "ni" (扫描码 49, 23)
        let act1 = session.handle_key_press(49, 1000, None);
        assert_eq!(act1, ImeAction::Handled);
        let act2 = session.handle_key_press(23, 1001, None);
        assert_eq!(act2, ImeAction::Handled);
        assert_eq!(session.engine.raw_input(), "ni");

        // 空格确认首选词 "你"
        let act_space = session.handle_key_press(KEY_SPACE, 1002, None);
        assert_eq!(act_space, ImeAction::Commit("你".to_string()));
        assert!(session.engine.raw_input().is_empty());
    }

    #[test]
    fn test_ime_core_session_capslock_commit_raw() {
        let mut session = make_test_session();
        session.handle_key_press(49, 1000, None);
        session.handle_key_press(23, 1001, None);
        assert_eq!(session.engine.raw_input(), "ni");

        // 按下 CapsLock，应立即提交原拼音 "ni"
        let (consumed, committed) = session.handle_capslock(true);
        assert!(!consumed);
        assert_eq!(committed.as_deref(), Some("ni"));
        assert!(session.engine.raw_input().is_empty());
    }

    #[test]
    fn test_ime_core_session_idle_punctuation() {
        let mut session = make_test_session();
        // 空闲状态下按逗号 (KEY_COMMA: 51)
        let act = session.handle_key_press(51, 1000, None);
        assert_eq!(act, ImeAction::Commit("，".to_string()));
    }
}
