//! IBus / GDK 按键符号与 Linux evdev 扫描码映射转换
//!
//! # 设计原理
//! - **实现初衷**：IBus 事件传入的是 X11/GDK keyval 与位掩码，内核按键引擎基于 Linux evdev 扫描码工作。
//!   本模块提供无损快速查表与修饰键掩码提取。
//! - **核心优势**：优先基于 keyval 进行字母与标点映射，自动适配非 QWERTY 键盘布局与 XWayland 窗口。
//! - **代价与局限**：非常规特殊多媒体键需要保留 fallback_keycode 回退。

use waylan_core::input::*;

/// 字母 F 的 evdev 扫描码 (33)
pub const KEY_F: u32 = 33;
/// 字母 T 的 evdev 扫描码 (20)
pub const KEY_T: u32 = 20;

/// IBus Shift 修饰位掩码 (`IBUS_SHIFT_MASK`)
pub const IBUS_SHIFT_MASK: u32 = 1 << 0;
/// IBus Lock/CapsLock 修饰位掩码 (`IBUS_LOCK_MASK`)
pub const IBUS_LOCK_MASK: u32 = 1 << 1;
/// IBus Ctrl 修饰位掩码 (`IBUS_CONTROL_MASK`)
pub const IBUS_CONTROL_MASK: u32 = 1 << 2;
/// IBus Alt 修饰位掩码 (`IBUS_MOD1_MASK`)
pub const IBUS_MOD1_MASK: u32 = 1 << 3;
/// IBus Super 修饰位掩码 (`IBUS_SUPER_MASK`)
pub const IBUS_SUPER_MASK: u32 = 1 << 26;
/// IBus Hyper 修饰位掩码 (`IBUS_HYPER_MASK`)
pub const IBUS_HYPER_MASK: u32 = 1 << 27;
/// IBus Meta 修饰位掩码 (`IBUS_META_MASK`)
pub const IBUS_META_MASK: u32 = 1 << 28;
/// IBus 按键释放标志位掩码 (`IBUS_RELEASE_MASK`)
pub const IBUS_RELEASE_MASK: u32 = 1 << 30;

/// GDK/IBus 左方向键 KeySym (`IBUS_KEY_Left`)
pub const IBUS_KEYVAL_LEFT: u32 = 0xff51;
/// GDK/IBus 右方向键 KeySym (`IBUS_KEY_Right`)
pub const IBUS_KEYVAL_RIGHT: u32 = 0xff53;

/// 判断 IBus 修饰位掩码是否包含 Ctrl / Alt / Super / Meta 修饰键
#[inline]
pub fn ibus_state_has_ctrl_or_alt(state: u32) -> bool {
    (state
        & (IBUS_CONTROL_MASK | IBUS_MOD1_MASK | IBUS_SUPER_MASK | IBUS_HYPER_MASK | IBUS_META_MASK))
        != 0
}

/// 将 IBus/GDK `keyval` 归一化为 Linux evdev 扫描码（未命中时回退至 `fallback_keycode`）
pub fn ibus_keyval_to_evdev(keyval: u32, fallback_keycode: u32) -> u32 {
    if let Some(code) = map_alpha_or_digit_keyval(keyval) {
        return code;
    }
    if let Some(code) = map_punct_or_control_keyval(keyval) {
        return code;
    }
    fallback_keycode
}

/// 映射字母与数字相关的 `keyval` 到 evdev 扫描码
fn map_alpha_or_digit_keyval(keyval: u32) -> Option<u32> {
    let lower = if (0x41..=0x5a).contains(&keyval) {
        keyval + 32
    } else {
        keyval
    };
    let code = match lower {
        0x61 => 30, // a
        0x62 => 48, // b
        0x63 => 46, // c
        0x64 => 32, // d
        0x65 => 18, // e
        0x66 => 33, // f
        0x67 => 34, // g
        0x68 => 35, // h
        0x69 => 23, // i
        0x6a => 36, // j
        0x6b => 37, // k
        0x6c => 38, // l
        0x6d => 50, // m
        0x6e => 49, // n
        0x6f => 24, // o
        0x70 => 25, // p
        0x71 => 16, // q
        0x72 => 19, // r
        0x73 => 31, // s
        0x74 => 20, // t
        0x75 => 22, // u
        0x76 => 47, // v
        0x77 => 17, // w
        0x78 => 45, // x
        0x79 => 21, // y
        0x7a => 44, // z
        0x31 | 0x21 => KEY_1,
        0x32 | 0x40 => KEY_2,
        0x33 | 0x23 => KEY_3,
        0x34 | 0x24 => KEY_4,
        0x35 | 0x25 => KEY_5,
        0x36 | 0x5e => KEY_6,
        0x37 | 0x26 => KEY_7,
        0x38 | 0x2a => KEY_8,
        0x39 | 0x28 => KEY_9,
        0x30 | 0x29 => KEY_0,
        _ => return None,
    };
    Some(code)
}

/// 映射标点、导航、修饰键及小键盘的 `keyval` 到 evdev 扫描码
fn map_punct_or_control_keyval(keyval: u32) -> Option<u32> {
    let code = match keyval {
        0x0020 => KEY_SPACE,
        0x002d | 0x005f => KEY_MINUS,
        0x003d | 0x002b => KEY_EQUAL,
        0x005b | 0x007b => KEY_LEFTBRACE,
        0x005d | 0x007d => KEY_RIGHTBRACE,
        0x003b | 0x003a => KEY_SEMICOLON,
        0x0027 | 0x0022 => KEY_APOSTROPHE,
        0x0060 | 0x007e => KEY_GRAVE,
        0x002c | 0x003c => KEY_COMMA,
        0x002e | 0x003e => KEY_DOT,
        0x002f | 0x003f => KEY_SLASH,
        0xff08 => KEY_BACKSPACE,
        0xff09 | 0xfe20 => KEY_TAB,
        0xff0d => KEY_ENTER,
        0xff8d => KEY_KPENTER,
        0xff1b => KEY_ESC,
        0xffff => KEY_DELETE,
        0xff51 => KEY_LEFT,
        0xff52 => KEY_UP,
        0xff53 => KEY_RIGHT,
        0xff54 => KEY_DOWN,
        0xff55 => KEY_PAGEUP,
        0xff56 => KEY_PAGEDOWN,
        0xffe1 => KEY_LEFTSHIFT,
        0xffe2 => KEY_RIGHTSHIFT,
        0xffe3 => KEY_LEFTCTRL,
        0xffe4 => KEY_RIGHTCTRL,
        0xffe9 => KEY_LEFTALT,
        0xffea => KEY_RIGHTALT,
        0xffeb => KEY_LEFTMETA,
        0xffec => KEY_RIGHTMETA,
        0xffe5 | 0xffe6 => KEY_CAPSLOCK,
        0xffb0 => KEY_KP0,
        0xffb1 => KEY_KP1,
        0xffb2 => KEY_KP2,
        0xffb3 => KEY_KP3,
        0xffb4 => KEY_KP4,
        0xffb5 => KEY_KP5,
        0xffb6 => KEY_KP6,
        0xffb7 => KEY_KP7,
        0xffb8 => KEY_KP8,
        0xffb9 => KEY_KP9,
        0xffaa => KEY_KPASTERISK,
        0xffab => KEY_KPPLUS,
        0xffad => KEY_KPMINUS,
        0xffae => KEY_KPDOT,
        0xffaf => KEY_KPSLASH,
        _ => return None,
    };
    Some(code)
}
