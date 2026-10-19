//! 输入法核心通用会话管理与按键驱动状态机
//!
//! # 设计原理
//! - **实现初衷**：消除 Wayland V1、Wayland V2 与 IBus 等多协议端在按键分流、
//!   CapsLock 截断提交、中英 Shift 模式切换与智能成对标点处理上的样板代码重复。
//! - **核心优势**：纯逻辑抽象，零特定协议依赖；单一状态真实源（Single Source of Truth）；
//!   多协议端共享相同业务输入法决策，彻底规避平台行为不一致与维护失步。
//! - **代价与局限**：协议适配层需消费本模块产出的 [`ImeAction`] 或 [`KeyProcessResult`] 动作，
//!   增加了单层轻量结果解构，但彻底隔绝了平台底层对复杂打字业务逻辑的污染。

pub mod dispatch;
pub mod grid;
pub mod key_handler;
#[cfg(test)]
mod tests;
pub mod types;
pub mod viewport;

pub use dispatch::KeyProcessContext;
pub use key_handler::{handle_composition_key_with_params, CompositionKeyParams};
pub use types::*;

use crate::input::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::AlgoEngine;
use waylan_core::input::*;
use waylan_core::{CandidateExpansion, InputMode, ParsedShortcut};

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
    /// 简繁即时切换快捷键选项
    pub traditional_shortcut: Option<ParsedShortcut>,
    /// 字符全角/半角切换快捷键选项
    pub full_width_char_shortcut: Option<ParsedShortcut>,
    /// 标点全角/半角切换快捷键选项
    pub full_width_punct_shortcut: Option<ParsedShortcut>,
    /// 输入修饰扩展状态（繁体输出、全角标点、全角字符）
    pub decoration_flags: InputDecorationFlags,
    /// 当前候选词分页索引（0-indexed）
    pub page_index: usize,
    /// 当前页内选中的高亮候选词局部索引（0-indexed）
    pub selected_index: usize,
    /// 候选框单页展示的最大候选词数量
    pub page_size: usize,
    /// 候选框展开交互状态机（大卡片与多行大网格在类型层面严格互斥）
    pub expansion: CandidateExpansion,
}

impl Default for ImeCoreSession {
    fn default() -> Self {
        Self::new(AlgoEngine::with_sample_data(
            waylan_algo::types::AlgoConfig::default(),
        ))
    }
}

impl ImeCoreSession {
    /// 创建带有指定算法引擎的核心会话
    pub fn new(engine: AlgoEngine) -> Self {
        let page_size = engine.config.page_size.max(1);
        let traditional_chinese = engine.config.traditional_chinese;
        let punctuation_processor = PunctuationProcessor::new();
        let full_width_punct = punctuation_processor.is_full_width_punct();
        Self {
            engine,
            shift_sm: ShiftToggleStateMachine::new(),
            punctuation_processor,
            page_keys: PageKeyConfig::default(),
            keymap_layout: KeymapLayout::default(),
            mode: InputMode::Chinese,
            consumed_keys: ConsumedKeyTracker::new(),
            modifiers: ModifierState::default(),
            traditional_shortcut: ParsedShortcut::parse("Ctrl + Shift + F"),
            full_width_char_shortcut: ParsedShortcut::parse("Shift + Space"),
            full_width_punct_shortcut: ParsedShortcut::parse("Ctrl + ."),
            decoration_flags: InputDecorationFlags::new(
                traditional_chinese,
                full_width_punct,
                false,
            ),
            page_index: 0,
            selected_index: 0,
            page_size,
            expansion: CandidateExpansion::None,
        }
    }

    /// 重置会话交互状态（大网格与二级卡片均收起，并清空算法组词缓冲区）
    pub fn reset_context(&mut self) {
        self.reset_viewport();
        self.engine.reset();
    }

    /// 获取当前输入修饰扩展状态快照
    #[inline]
    pub fn decoration_flags(&self) -> InputDecorationFlags {
        self.decoration_flags
    }

    /// 批量设置输入修饰扩展状态并同步至底层组件
    pub fn set_decoration_flags(&mut self, flags: InputDecorationFlags) {
        self.decoration_flags = flags;
        self.engine.config.traditional_chinese = flags.traditional_chinese;
        self.punctuation_processor
            .set_full_width_punct(flags.full_width_punct);
    }

    /// 查询当前是否处于全角字符模式
    #[inline]
    pub fn is_full_width_char(&self) -> bool {
        self.decoration_flags.full_width_char
    }

    /// 设置全角字符模式
    #[inline]
    pub fn set_full_width_char(&mut self, enabled: bool) {
        self.decoration_flags.full_width_char = enabled;
    }

    /// 获取当前输入模式
    #[inline]
    pub fn input_mode(&self) -> InputMode {
        self.mode
    }

    /// 设置输入模式
    #[inline]
    pub fn set_input_mode(&mut self, mode: InputMode) {
        self.mode = mode;
    }

    /// 查询当前是否开启繁体中文输出
    #[inline]
    pub fn is_traditional(&self) -> bool {
        self.decoration_flags.traditional_chinese
    }

    /// 设置繁体中文输出模式
    #[inline]
    pub fn set_traditional(&mut self, enabled: bool) {
        self.decoration_flags.traditional_chinese = enabled;
        self.engine.config.traditional_chinese = enabled;
    }

    /// 查询当前是否开启全角标点
    #[inline]
    pub fn is_full_width_punct(&self) -> bool {
        self.decoration_flags.full_width_punct
    }

    /// 设置全角标点模式
    #[inline]
    pub fn set_full_width_punct(&mut self, enabled: bool) {
        self.decoration_flags.full_width_punct = enabled;
        self.punctuation_processor.set_full_width_punct(enabled);
    }

    /// 设置 CapsLock 锁定状态
    pub fn set_caps_lock(&mut self, is_locked: bool) {
        if is_locked {
            self.modifiers.locked |= MOD_CAPS;
        } else {
            self.modifiers.locked &= !MOD_CAPS;
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

    /// 重置输入状态（清空修饰键、消费追踪、Shift 状态机、标点处理器、网格展开态与算法引擎）
    pub fn reset_input_state(&mut self) {
        self.modifiers.reset();
        self.consumed_keys.clear();
        self.shift_sm.reset();
        self.punctuation_processor.reset();
        self.collapse_grid();
        self.collapse_card();
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

    /// 匹配并执行用户配置的全局模式切换快捷键（简繁切换、字符全半角、标点全半角）
    pub fn handle_global_shortcuts(&mut self, keycode: u32) -> Option<ShortcutToggleAction> {
        let shift_active = self.is_shift_active();
        let ctrl_active = (self.modifiers.depressed & MOD_CTRL) != 0;
        let alt_active = (self.modifiers.depressed & MOD_ALT) != 0;
        let super_active = (self.modifiers.depressed & MOD_SUPER) != 0;

        // 1. 尝试匹配简繁切换快捷键
        if self.traditional_shortcut.as_ref().is_some_and(|s| {
            s.matches(keycode, shift_active, ctrl_active, alt_active, super_active)
        }) {
            let next_val = !self.is_traditional();
            self.set_traditional(next_val);
            return Some(ShortcutToggleAction::Traditional(next_val));
        }

        // 2. 尝试匹配字符全半角切换快捷键
        if self.full_width_char_shortcut.as_ref().is_some_and(|s| {
            s.matches(keycode, shift_active, ctrl_active, alt_active, super_active)
        }) {
            let next_val = !self.is_full_width_char();
            self.set_full_width_char(next_val);
            return Some(ShortcutToggleAction::FullWidthChar(next_val));
        }

        // 3. 尝试匹配标点全半角切换快捷键
        if self.full_width_punct_shortcut.as_ref().is_some_and(|s| {
            s.matches(keycode, shift_active, ctrl_active, alt_active, super_active)
        }) {
            let next_val = !self.is_full_width_punct();
            self.set_full_width_punct(next_val);
            return Some(ShortcutToggleAction::FullWidthPunct(next_val));
        }

        None
    }
}

/// 从周围文本元组 (text, cursor, anchor) 中安全提取光标前至多 `max_bytes` 字节的上下文切片
pub fn extract_surrounding_prefix(
    surrounding: Option<&(String, u32, u32)>,
    max_bytes: usize,
) -> Option<&str> {
    let (text, cursor, _) = surrounding?;
    let len = text.len();
    let mut cursor_idx = (*cursor as usize).min(len);
    while cursor_idx > 0 && !text.is_char_boundary(cursor_idx) {
        cursor_idx -= 1;
    }
    let start = cursor_idx.saturating_sub(max_bytes);
    let mut start_idx = start.min(len);
    while start_idx < len && !text.is_char_boundary(start_idx) {
        start_idx += 1;
    }
    Some(&text[start_idx..cursor_idx])
}
