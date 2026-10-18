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

pub use dispatch::KeyProcessContext;
pub use key_handler::{
    handle_composition_key, handle_composition_key_with_params, CompositionKeyParams,
};

use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::AlgoEngine;
use waylan_core::input::*;
use waylan_core::{FullWidthCharShortcut, FullWidthPunctShortcut, InputMode, TraditionalShortcut};

/// 全局模式与功能快捷键切换动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutToggleAction {
    /// 简繁即时切换，包含切换后的布尔值（true 为繁体中文）
    Traditional(bool),
    /// 字符全角/半角切换，包含切换后的布尔值（true 为全角字符）
    FullWidthChar(bool),
    /// 标点全角/半角切换，包含切换后的布尔值（true 为全角标点）
    FullWidthPunct(bool),
}

/// 核心输入法按键与会话事件统一决策动作 (IME Output Action)
///
/// # 设计原理
/// - **消除概念膨胀**：统一拼音组词状态机、标点处理器、快捷键流转与各协议适配层的动作契约，
///   彻底清除原 `KeyProcessAction`、`ImeAction` 与 `KeyProcessResult` 之间的多层机械映射与中间层。
/// - **单一真相源**：单一动作枚举完整表达所有可能的输入法决策，上层协议（Wayland V1、V2、IBus）直接消费。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeOutputAction {
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
    pub traditional_shortcut: TraditionalShortcut,
    /// 字符全角/半角切换快捷键选项
    pub full_width_char_shortcut: FullWidthCharShortcut,
    /// 标点全角/半角切换快捷键选项
    pub full_width_punct_shortcut: FullWidthPunctShortcut,
    /// 是否开启全角字符/字母输入
    pub full_width_char: bool,
    /// 候选词是否处于大网格展开态
    pub is_grid_expanded: bool,
    /// 大网格展开态下的高亮选中项绝对索引 (0..30)
    pub grid_selected_index: usize,
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
        Self {
            engine,
            shift_sm: ShiftToggleStateMachine::new(),
            punctuation_processor: PunctuationProcessor::new(),
            page_keys: PageKeyConfig::default(),
            keymap_layout: KeymapLayout::default(),
            mode: InputMode::Chinese,
            consumed_keys: ConsumedKeyTracker::new(),
            modifiers: ModifierState::default(),
            traditional_shortcut: TraditionalShortcut::default(),
            full_width_char_shortcut: FullWidthCharShortcut::default(),
            full_width_punct_shortcut: FullWidthPunctShortcut::default(),
            full_width_char: false,
            is_grid_expanded: false,
            grid_selected_index: 0,
        }
    }

    /// 查询当前是否处于全角字符模式
    #[inline]
    pub fn is_full_width_char(&self) -> bool {
        self.full_width_char
    }

    /// 设置全角字符模式
    #[inline]
    pub fn set_full_width_char(&mut self, enabled: bool) {
        self.full_width_char = enabled;
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

        // 1. 尝试匹配简繁切换快捷键
        let trad_match = match self.traditional_shortcut {
            TraditionalShortcut::CtrlShiftF => {
                ctrl_active && shift_active && !alt_active && keycode == KEY_F
            }
            TraditionalShortcut::CtrlAltF => {
                ctrl_active && alt_active && !shift_active && keycode == KEY_F
            }
            TraditionalShortcut::CtrlShiftT => {
                ctrl_active && shift_active && !alt_active && keycode == KEY_T
            }
            TraditionalShortcut::Disabled => false,
        };
        if trad_match {
            let next_val = !self.engine.config.traditional_chinese;
            self.engine.config.traditional_chinese = next_val;
            return Some(ShortcutToggleAction::Traditional(next_val));
        }

        // 2. 尝试匹配字符全半角切换快捷键
        let char_match = match self.full_width_char_shortcut {
            FullWidthCharShortcut::ShiftSpace => {
                shift_active && !ctrl_active && !alt_active && keycode == KEY_SPACE
            }
            FullWidthCharShortcut::CtrlShiftSpace => {
                ctrl_active && shift_active && !alt_active && keycode == KEY_SPACE
            }
            FullWidthCharShortcut::Disabled => false,
        };
        if char_match {
            let next_val = !self.full_width_char;
            self.full_width_char = next_val;
            return Some(ShortcutToggleAction::FullWidthChar(next_val));
        }

        // 3. 尝试匹配标点全半角切换快捷键
        let punct_match = match self.full_width_punct_shortcut {
            FullWidthPunctShortcut::CtrlPeriod => {
                ctrl_active && !shift_active && !alt_active && keycode == KEY_DOT
            }
            FullWidthPunctShortcut::AltPeriod => {
                alt_active && !ctrl_active && !shift_active && keycode == KEY_DOT
            }
            FullWidthPunctShortcut::Disabled => false,
        };
        if punct_match {
            let next_val = !self.punctuation_processor.full_width_punct;
            self.punctuation_processor.full_width_punct = next_val;
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
