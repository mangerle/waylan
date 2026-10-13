//! Linux 输入子系统按键扫描码与修饰键位掩码常量定义
//!
//! 键码定义严格对标 `<linux/input-event-codes.h>`，供 Wayland 客户端与合成器协议会话映射使用。

/// Linux 常见按键键码常量（基于 linux/input-event-codes.h）
pub const KEY_ESC: u32 = 1;
pub const KEY_MINUS: u32 = 12;
pub const KEY_EQUAL: u32 = 13;
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_TAB: u32 = 15;
pub const KEY_ENTER: u32 = 28;
pub const KEY_SPACE: u32 = 57;
pub const KEY_CAPSLOCK: u32 = 58;
pub const KEY_LEFTSHIFT: u32 = 42;
pub const KEY_RIGHTSHIFT: u32 = 54;
pub const KEY_UP: u32 = 103;
pub const KEY_PAGEUP: u32 = 104;
pub const KEY_LEFT: u32 = 105;
pub const KEY_RIGHT: u32 = 106;
pub const KEY_DOWN: u32 = 108;
pub const KEY_PAGEDOWN: u32 = 109;
pub const KEY_DELETE: u32 = 111;
pub const KEY_SLASH: u32 = 53;
pub const KEY_COMMA: u32 = 51;
pub const KEY_DOT: u32 = 52;
pub const KEY_SEMICOLON: u32 = 39;
pub const KEY_APOSTROPHE: u32 = 40;
pub const KEY_LEFTBRACE: u32 = 26;
pub const KEY_RIGHTBRACE: u32 = 27;

pub const KEY_1: u32 = 2;
pub const KEY_2: u32 = 3;
pub const KEY_3: u32 = 4;
pub const KEY_4: u32 = 5;
pub const KEY_5: u32 = 6;
pub const KEY_6: u32 = 7;
pub const KEY_7: u32 = 8;
pub const KEY_8: u32 = 9;
pub const KEY_9: u32 = 10;
pub const KEY_0: u32 = 11;
pub const KEY_GRAVE: u32 = 41;
pub const KEY_LEFTCTRL: u32 = 29;
pub const KEY_RIGHTCTRL: u32 = 97;
pub const KEY_LEFTALT: u32 = 56;
pub const KEY_RIGHTALT: u32 = 100;
pub const KEY_LEFTMETA: u32 = 125;
pub const KEY_RIGHTMETA: u32 = 126;

/// 小键盘（Numpad）扫描码常量（对标 linux/input-event-codes.h）
pub const KEY_KP7: u32 = 71;
pub const KEY_KP8: u32 = 72;
pub const KEY_KP9: u32 = 73;
pub const KEY_KPMINUS: u32 = 74;
pub const KEY_KP4: u32 = 75;
pub const KEY_KP5: u32 = 76;
pub const KEY_KP6: u32 = 77;
pub const KEY_KPPLUS: u32 = 78;
pub const KEY_KP1: u32 = 79;
pub const KEY_KP2: u32 = 80;
pub const KEY_KP3: u32 = 81;
pub const KEY_KP0: u32 = 82;
pub const KEY_KPDOT: u32 = 83;
pub const KEY_KPENTER: u32 = 96;
pub const KEY_KPSLASH: u32 = 98;
pub const KEY_KPASTERISK: u32 = 55;
pub const KEY_KPEQUAL: u32 = 117;

/// 标准 XKB 修饰键位掩码常量
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CAPS: u32 = 2;
pub const MOD_CTRL: u32 = 4;
pub const MOD_ALT: u32 = 8;
pub const MOD_SUPER: u32 = 64;
