pub use crate::input_constants::*;
use waylan_algo::preprocessor::PunctuationProcessor;
use waylan_algo::AlgoEngine;

/// 候选词翻页快捷键配置模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageKeyConfig {
    /// 减号/等号翻页 (-/=)
    #[default]
    MinusEqual,
    /// 方括号翻页 ([/])
    Bracket,
    /// 逗号/句号翻页 (,/.)
    CommaPeriod,
}

impl PageKeyConfig {
    /// 判断给定的键码是否对应向上一页翻页操作
    pub fn is_prev_page(&self, keycode: u32) -> bool {
        match self {
            Self::MinusEqual => keycode == KEY_MINUS || keycode == KEY_PAGEUP,
            Self::Bracket => keycode == KEY_LEFTBRACE || keycode == KEY_PAGEUP,
            Self::CommaPeriod => keycode == KEY_COMMA || keycode == KEY_PAGEUP,
        }
    }

    /// 判断给定的键码是否对应向下一页翻页操作
    pub fn is_next_page(&self, keycode: u32) -> bool {
        match self {
            Self::MinusEqual => keycode == KEY_EQUAL || keycode == KEY_PAGEDOWN,
            Self::Bracket => keycode == KEY_RIGHTBRACE || keycode == KEY_PAGEDOWN,
            Self::CommaPeriod => keycode == KEY_DOT || keycode == KEY_PAGEDOWN,
        }
    }
}

/// 判断按键是否为向上翻页键
#[inline]
pub fn is_prev_page_key(keycode: u32, page_keys: PageKeyConfig) -> bool {
    page_keys.is_prev_page(keycode)
}

/// 判断按键是否为向下翻页键
#[inline]
pub fn is_next_page_key(keycode: u32, page_keys: PageKeyConfig) -> bool {
    page_keys.is_next_page(keycode)
}

pub use crate::keymap::KeymapLayout;

/// 将 Linux 扫描码转换为 ASCII 小写英文字母（基于默认 QWERTY 布局）
pub fn keycode_to_char(keycode: u32) -> Option<char> {
    KeymapLayout::qwerty().get_char(keycode)
}

/// 根据指定键盘布局将 Linux 扫描码转换为 ASCII 小写英文字母
pub fn keycode_to_char_with_layout(keycode: u32, layout: &KeymapLayout) -> Option<char> {
    layout.get_char(keycode)
}

/// 将扫描码转换为 1-indexed 选词序号（1..=9 对应 1..=9 号候选，0 对应第 10 号候选，支持主键盘与小键盘）
pub fn keycode_to_digit(keycode: u32) -> Option<usize> {
    match keycode {
        KEY_1 | KEY_KP1 => Some(1),
        KEY_2 | KEY_KP2 => Some(2),
        KEY_3 | KEY_KP3 => Some(3),
        KEY_4 | KEY_KP4 => Some(4),
        KEY_5 | KEY_KP5 => Some(5),
        KEY_6 | KEY_KP6 => Some(6),
        KEY_7 | KEY_KP7 => Some(7),
        KEY_8 | KEY_KP8 => Some(8),
        KEY_9 | KEY_KP9 => Some(9),
        KEY_0 | KEY_KP0 => Some(10),
        _ => None,
    }
}

/// 将数字键扫描码转换为对应的 ASCII 数字字符 ('0'..='9')，支持主键盘与小键盘
pub fn keycode_to_ascii_digit_char(keycode: u32) -> Option<char> {
    match keycode {
        KEY_1 | KEY_KP1 => Some('1'),
        KEY_2 | KEY_KP2 => Some('2'),
        KEY_3 | KEY_KP3 => Some('3'),
        KEY_4 | KEY_KP4 => Some('4'),
        KEY_5 | KEY_KP5 => Some('5'),
        KEY_6 | KEY_KP6 => Some('6'),
        KEY_7 | KEY_KP7 => Some('7'),
        KEY_8 | KEY_KP8 => Some('8'),
        KEY_9 | KEY_KP9 => Some('9'),
        KEY_0 | KEY_KP0 => Some('0'),
        _ => None,
    }
}

/// 将扫描码转换为常见中文标点符号字符（未按 Shift）
pub fn keycode_to_punctuation(keycode: u32) -> Option<char> {
    match keycode {
        KEY_COMMA => Some(','),
        KEY_DOT => Some('.'),
        KEY_SLASH => Some('/'),
        KEY_SEMICOLON => Some(';'),
        KEY_APOSTROPHE => Some('\''),
        KEY_LEFTBRACE => Some('['),
        KEY_RIGHTBRACE => Some(']'),
        _ => None,
    }
}

/// 结合 Shift 修饰键状态将扫描码转换为中文标点源字符（支持 !?$^_()""<>{}:~ 等 Shift 标点）
pub fn keycode_to_punctuation_with_shift(keycode: u32, shift_pressed: bool) -> Option<char> {
    if shift_pressed {
        match keycode {
            KEY_1 => Some('!'),
            KEY_4 => Some('$'),
            KEY_6 => Some('^'),
            KEY_SLASH => Some('?'),
            KEY_9 => Some('('),
            KEY_0 => Some(')'),
            KEY_MINUS => Some('_'),
            KEY_APOSTROPHE => Some('"'),
            KEY_COMMA => Some('<'),
            KEY_DOT => Some('>'),
            KEY_LEFTBRACE => Some('{'),
            KEY_RIGHTBRACE => Some('}'),
            KEY_SEMICOLON => Some(':'),
            KEY_GRAVE => Some('~'),
            _ => None,
        }
    } else {
        keycode_to_punctuation(keycode)
    }
}

/// 将键盘扫描码及 Shift 修饰状态映射为 V 模式（算式与数字转换）所支持的输入字符
///
/// # 映射规则
/// - 算术运算与括号：
///   - '+'：Shift+KEY_EQUAL 或 KEY_KPPLUS
///   - '-'：KEY_MINUS（未按 Shift）或 KEY_KPMINUS
///   - '*'：Shift+KEY_8 或 KEY_KPASTERISK
///   - '/'：KEY_SLASH（未按 Shift）或 KEY_KPSLASH
///   - '%'：Shift+KEY_5
///   - '('：Shift+KEY_9；')'：Shift+KEY_0
///   - '.'：KEY_DOT（未按 Shift）或 KEY_KPDOT
/// - 数字字符（'0'..='9'）：主键盘与小键盘数字键（未按 Shift）
pub fn keycode_to_v_mode_char(keycode: u32, shift_pressed: bool) -> Option<char> {
    if shift_pressed {
        match keycode {
            KEY_EQUAL => Some('+'),
            KEY_8 => Some('*'),
            KEY_5 => Some('%'),
            KEY_9 => Some('('),
            KEY_0 => Some(')'),
            KEY_KPPLUS => Some('+'),
            KEY_KPMINUS => Some('-'),
            KEY_KPASTERISK => Some('*'),
            KEY_KPSLASH => Some('/'),
            KEY_KPDOT => Some('.'),
            _ => None,
        }
    } else {
        match keycode {
            KEY_MINUS | KEY_KPMINUS => Some('-'),
            KEY_SLASH | KEY_KPSLASH => Some('/'),
            KEY_DOT | KEY_KPDOT => Some('.'),
            KEY_KPPLUS => Some('+'),
            KEY_KPASTERISK => Some('*'),
            _ => keycode_to_ascii_digit_char(keycode),
        }
    }
}

/// 核心按键决策结果
///
/// # 设计原理
/// - **实现初衷**：解耦按键消费决策与具体的 Wayland 协议版本（V1 zwp_input_method_v1 与 V2 zwp_input_method_v2），
///   将拼音预编辑与候选词选择的通用状态机决策收敛至协议无关的核心层。
/// - **核心优势**：消除 V1 和 V2 之间数百行高度重合的按键分支代码，防止逻辑分叉。
#[derive(Debug, PartialEq, Eq)]
pub enum KeyProcessAction {
    /// 原样透传给客户端应用
    PassThrough,
    /// 内部消费，刷新候选表面（无需提交）
    Handled,
    /// 提交整段文字上屏，并隐藏候选表面与清空预编辑
    Commit(String),
    /// 提交部分文字，并保留/刷新剩余拼音预编辑
    CommitPartial {
        committed: String,
        remaining_preedit: String,
    },
    /// 清空预编辑并重置状态（如 ESC）
    Reset,
}

/// 拼音组词状态按键决策输入参数
#[derive(Debug, Clone, Copy)]
pub struct CompositionKeyParams {
    pub key: u32,
    pub shift_active: bool,
    pub page_keys: PageKeyConfig,
    pub now: u64,
}

/// 处理二级展开卡片导航或常规上下候选词高亮切换
fn handle_expanded_or_arrow_key(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
) -> Option<KeyProcessAction> {
    if key == KEY_TAB {
        let sel = engine.selected_index;
        engine.toggle_expand(sel);
        return Some(KeyProcessAction::Handled);
    }

    if engine.is_expanded {
        let sel = engine.selected_index;
        if key == KEY_DOWN {
            engine.next_expanded_item(sel);
            return Some(KeyProcessAction::Handled);
        }
        if key == KEY_UP {
            engine.prev_expanded_item(sel);
            return Some(KeyProcessAction::Handled);
        }
        if key == KEY_ENTER || key == KEY_KPENTER || key == KEY_SPACE {
            if let Some((committed, _)) =
                engine.select_expanded_item(sel, engine.expanded_selected_index)
            {
                punct_processor.record_commit(&committed);
                return Some(KeyProcessAction::Commit(committed));
            }
        }
    } else {
        if key == KEY_DOWN {
            engine.next_candidate();
            return Some(KeyProcessAction::Handled);
        }
        if key == KEY_UP {
            engine.prev_candidate();
            return Some(KeyProcessAction::Handled);
        }
    }
    None
}

/// 选取指定索引候选词并转换为协议决策枚举
fn select_candidate_action(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    cand_idx: usize,
    now: u64,
) -> KeyProcessAction {
    if let Some((committed_text, remaining_len)) = engine.select_candidate(cand_idx, now) {
        punct_processor.record_commit(&committed_text);
        if remaining_len > 0 {
            KeyProcessAction::CommitPartial {
                committed: committed_text,
                remaining_preedit: engine.formatted_preedit(),
            }
        } else {
            KeyProcessAction::Commit(committed_text)
        }
    } else {
        KeyProcessAction::Handled
    }
}

/// 使用收敛的参数结构体处理处于拼音组词状态（raw_input 非空）下的核心按键拦截决策
pub fn handle_composition_key_with_params(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    params: CompositionKeyParams,
) -> Option<KeyProcessAction> {
    let CompositionKeyParams {
        key,
        shift_active,
        page_keys,
        now,
    } = params;

    if let Some(act) = handle_expanded_or_arrow_key(engine, punct_processor, key) {
        return Some(act);
    }

    if engine.raw_input().starts_with('v') {
        if let Some(ch) = keycode_to_v_mode_char(key, shift_active) {
            engine.push_char(ch);
            return Some(KeyProcessAction::Handled);
        }
    } else if !shift_active {
        if let Some(digit) = keycode_to_digit(key) {
            return Some(select_candidate_action(
                engine,
                punct_processor,
                digit.saturating_sub(1),
                now,
            ));
        }
    }

    if key == KEY_SPACE {
        let sel = engine.selected_index;
        return Some(select_candidate_action(engine, punct_processor, sel, now));
    }
    if key == KEY_ENTER || key == KEY_KPENTER {
        let raw = engine.raw_input().to_string();
        engine.reset();
        return Some(KeyProcessAction::Commit(raw));
    }
    if key == KEY_BACKSPACE || key == KEY_DELETE {
        if key == KEY_BACKSPACE {
            engine.pop_char();
        } else {
            engine.delete_char_at_cursor();
        }
        return Some(if engine.raw_input().is_empty() {
            KeyProcessAction::Reset
        } else {
            KeyProcessAction::Handled
        });
    }
    if !shift_active && key == KEY_APOSTROPHE {
        engine.push_char('\'');
        return Some(KeyProcessAction::Handled);
    }
    if !shift_active && (page_keys.is_prev_page(key) || page_keys.is_next_page(key)) {
        if page_keys.is_prev_page(key) {
            engine.prev_page();
        } else {
            engine.next_page();
        }
        return Some(KeyProcessAction::Handled);
    }
    if key == KEY_LEFT || key == KEY_RIGHT {
        if key == KEY_LEFT {
            engine.move_cursor_left();
        } else {
            engine.move_cursor_right();
        }
        return Some(KeyProcessAction::Handled);
    }
    if key == KEY_ESC {
        engine.reset();
        return Some(KeyProcessAction::Reset);
    }
    None
}

/// 处理处于拼音组词状态（raw_input 非空）下的核心按键拦截决策
pub fn handle_composition_key(
    engine: &mut AlgoEngine,
    punct_processor: &mut PunctuationProcessor,
    key: u32,
    shift_active: bool,
    page_keys: PageKeyConfig,
    now: u64,
) -> Option<KeyProcessAction> {
    handle_composition_key_with_params(
        engine,
        punct_processor,
        CompositionKeyParams {
            key,
            shift_active,
            page_keys,
            now,
        },
    )
}

/// 根据物理按键按下/释放事件独立维护左右修饰键状态并同步更新 XKB 位掩码
///
/// # 设计原理
/// - **实现初衷**：解决左右两侧同名修饰键（如 LeftShift 与 RightShift）同时按下后松开其中一侧导致整个修饰键标志位被提前清空的缺陷，并补全 Super/Meta 键跟踪。
/// - **核心优势**：使用 1 字节位掩码 `phys_mods` 独立记录 8 个左右物理修饰键，零堆内存分配。
pub fn update_modifier_masks(
    depressed: &mut u32,
    phys_mods: &mut u8,
    keycode: u32,
    is_press: bool,
) {
    let (phys_bit, group_bits, xkb_mask) = match keycode {
        KEY_LEFTSHIFT => (0b0000_0001u8, 0b0000_0011u8, MOD_SHIFT),
        KEY_RIGHTSHIFT => (0b0000_0010u8, 0b0000_0011u8, MOD_SHIFT),
        KEY_LEFTCTRL => (0b0000_0100u8, 0b0000_1100u8, MOD_CTRL),
        KEY_RIGHTCTRL => (0b0000_1000u8, 0b0000_1100u8, MOD_CTRL),
        KEY_LEFTALT => (0b0001_0000u8, 0b0011_0000u8, MOD_ALT),
        KEY_RIGHTALT => (0b0010_0000u8, 0b0011_0000u8, MOD_ALT),
        KEY_LEFTMETA => (0b0100_0000u8, 0b1100_0000u8, MOD_SUPER),
        KEY_RIGHTMETA => (0b1000_0000u8, 0b1100_0000u8, MOD_SUPER),
        _ => return,
    };

    if is_press {
        *phys_mods |= phys_bit;
        *depressed |= xkb_mask;
    } else {
        *phys_mods &= !phys_bit;
        if (*phys_mods & group_bits) == 0 {
            *depressed &= !xkb_mask;
        }
    }
}

/// 按键按下与释放对称消费跟踪器（基于栈位图的零分配实现）
///
/// # 设计原理
/// - **实现初衷**：确保输入法对同一个物理按键的 Press 与 Release 事件做出严格对称的拦截或透传决策。
///   防止：1) Shift 按下透传给客户端但松开触发中英切换被吞掉，导致客户端 Shift 卡死；
///   2) 空格/数字/回车按下完成选词清空预编辑后，松开事件因 `raw_input` 已空而泄漏给客户端触发误操作。
/// - **核心优势**：采用 `[u64; 12]` 栈上位图全量覆盖 Linux `KEY_MAX` (0x2ff/767) 全部键盘扫描码，查询与更新均为 $O(1)$ 位运算且无堆分配。
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsumedKeyTracker {
    bits: [u64; 12],
}

impl ConsumedKeyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录按键按下（Press）阶段是否被输入法内部消费
    #[inline]
    pub fn record_press(&mut self, keycode: u32, consumed: bool) {
        let idx = (keycode / 64) as usize;
        let bit = keycode % 64;
        if let Some(word) = self.bits.get_mut(idx) {
            if consumed {
                *word |= 1u64 << bit;
            } else {
                *word &= !(1u64 << bit);
            }
        }
    }

    /// 在按键释放（Release）阶段查询并清除该键的消费标记，返回按下阶段是否曾被输入法消费
    #[inline]
    pub fn take_release(&mut self, keycode: u32) -> bool {
        let idx = (keycode / 64) as usize;
        let bit = keycode % 64;
        if let Some(word) = self.bits.get_mut(idx) {
            let mask = 1u64 << bit;
            let was_consumed = (*word & mask) != 0;
            *word &= !mask;
            was_consumed
        } else {
            false
        }
    }

    /// 清空所有按键消费记录（用于会话激活、失焦或重置）
    #[inline]
    pub fn clear(&mut self) {
        self.bits = [0; 12];
    }
}

/// 共享修饰键状态跟踪器
#[derive(Debug, Default, Clone)]
pub struct ModifierState {
    pub depressed: u32,
    pub phys_mods: u8,
}

impl ModifierState {
    pub fn update(&mut self, keycode: u32, is_press: bool) {
        update_modifier_masks(&mut self.depressed, &mut self.phys_mods, keycode, is_press);
    }

    #[inline]
    pub fn is_shift_active(&self) -> bool {
        (self.depressed & MOD_SHIFT) != 0
    }

    #[inline]
    pub fn has_ctrl_or_alt(&self) -> bool {
        (self.depressed & (MOD_CTRL | MOD_ALT | MOD_SUPER)) != 0
    }

    #[inline]
    pub fn reset(&mut self) {
        self.depressed = 0;
        self.phys_mods = 0;
    }
}

pub use crate::fs_watcher::check_watched_paths_changed;

#[cfg(test)]
#[path = "input_core_tests.rs"]
mod tests;
