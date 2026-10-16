//! Linux 扫描码到字符、序号与标点符号的映射转换

use super::constants::{
    KEY_0, KEY_1, KEY_2, KEY_3, KEY_4, KEY_5, KEY_6, KEY_7, KEY_8, KEY_9, KEY_APOSTROPHE,
    KEY_COMMA, KEY_DOT, KEY_EQUAL, KEY_GRAVE, KEY_KP0, KEY_KP1, KEY_KP2, KEY_KP3, KEY_KP4, KEY_KP5,
    KEY_KP6, KEY_KP7, KEY_KP8, KEY_KP9, KEY_KPASTERISK, KEY_KPDOT, KEY_KPMINUS, KEY_KPPLUS,
    KEY_KPSLASH, KEY_LEFTBRACE, KEY_MINUS, KEY_RIGHTBRACE, KEY_SEMICOLON, KEY_SLASH,
};
use super::keymap::KeymapLayout;

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
