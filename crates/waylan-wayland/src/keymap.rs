//! 键盘字符布局映射管理（适配 QWERTY、Dvorak、Colemak、AZERTY 等动态 XKB 键盘布局）

use std::collections::HashMap;

use crate::input_constants::KEY_APOSTROPHE;

/// 键盘字符映射表（支持运行时动态从 XKB Keymap 文本更新，适配 Dvorak、Colemak、AZERTY 等非 QWERTY 布局）
#[derive(Debug, Clone)]
pub struct KeymapLayout {
    char_map: HashMap<u32, char>,
}

impl Default for KeymapLayout {
    fn default() -> Self {
        Self::qwerty()
    }
}

impl KeymapLayout {
    /// 使用显式字符映射表构造布局实例
    pub fn new(char_map: HashMap<u32, char>) -> Self {
        Self { char_map }
    }

    /// 标准 QWERTY 布局默认映射
    pub fn qwerty() -> Self {
        let mut map = HashMap::with_capacity(32);
        let pairs = [
            (30, 'a'),
            (48, 'b'),
            (46, 'c'),
            (32, 'd'),
            (18, 'e'),
            (33, 'f'),
            (34, 'g'),
            (35, 'h'),
            (23, 'i'),
            (36, 'j'),
            (37, 'k'),
            (38, 'l'),
            (50, 'm'),
            (49, 'n'),
            (24, 'o'),
            (25, 'p'),
            (16, 'q'),
            (19, 'r'),
            (31, 's'),
            (20, 't'),
            (22, 'u'),
            (47, 'v'),
            (17, 'w'),
            (45, 'x'),
            (21, 'y'),
            (44, 'z'),
            (KEY_APOSTROPHE, '\''),
        ];
        for (k, c) in pairs {
            map.insert(k, c);
        }
        Self { char_map: map }
    }

    /// 查询扫描码对应的 ASCII 小写字母
    #[inline]
    pub fn get_char(&self, keycode: u32) -> Option<char> {
        self.char_map.get(&keycode).copied()
    }

    /// 解析 XKB 字符定义并更新字符映射表（优先截取 xkb_symbols 段，防止被前置 xkb_keycodes 段拦截）
    pub fn update_from_xkb_text(&mut self, text: &str) {
        let symbols_section = text
            .find("xkb_symbols")
            .map(|idx| &text[idx..])
            .unwrap_or(text);

        let mut parsed_map = HashMap::with_capacity(32);
        for &(keyname, code) in EVDEV_KEYNAMES {
            if let Some(c) = extract_key_symbol(symbols_section, keyname) {
                parsed_map.insert(code, c);
            }
        }

        // 若成功解析出至少 10 个字母，则采用新映射并用 QWERTY 兜底补全
        if parsed_map.len() >= 10 {
            for (k, v) in Self::qwerty().char_map {
                parsed_map.entry(k).or_insert(v);
            }
            self.char_map = parsed_map;
        }
    }
}

/// 标准 evdev 按键名到 Linux 硬件扫描码的固定映射表
const EVDEV_KEYNAMES: &[(&str, u32)] = &[
    ("AD01", 16),
    ("AD02", 17),
    ("AD03", 18),
    ("AD04", 19),
    ("AD05", 20),
    ("AD06", 21),
    ("AD07", 22),
    ("AD08", 23),
    ("AD09", 24),
    ("AD10", 25),
    ("AC01", 30),
    ("AC02", 31),
    ("AC03", 32),
    ("AC04", 33),
    ("AC05", 34),
    ("AC06", 35),
    ("AC07", 36),
    ("AC08", 37),
    ("AC09", 38),
    ("AC10", 39),
    ("AB01", 44),
    ("AB02", 45),
    ("AB03", 46),
    ("AB04", 47),
    ("AB05", 48),
    ("AB06", 49),
    ("AB07", 50),
    ("AB08", 51),
    ("AB09", 52),
    ("AB10", 53),
    ("AC11", KEY_APOSTROPHE),
];

/// 从 XKB symbols 文本段中提取指定 evdev 按键名的首个有效小写 ASCII 字符
fn extract_key_symbol(section: &str, keyname: &str) -> Option<char> {
    let pattern = format!("<{}>", keyname);
    let mut rest = section;
    while let Some(pos) = rest.find(&pattern) {
        let after_key = &rest[pos + pattern.len()..];
        let Some(semi_pos) = after_key.find(';') else {
            break;
        };
        let key_block = &after_key[..semi_pos];
        if let Some(bracket_start) = key_block.find('[') {
            if let Some(bracket_end) = key_block[bracket_start..].find(']') {
                let syms = &key_block[bracket_start + 1..bracket_start + bracket_end];
                if let Some(first_sym) = syms.split(',').next() {
                    let sym = first_sym.trim();
                    if sym.len() == 1 {
                        let c = sym.chars().next().unwrap_or('\0').to_ascii_lowercase();
                        if c.is_ascii_alphabetic() || c == '\'' {
                            return Some(c);
                        }
                    }
                }
            }
        }
        rest = &after_key[semi_pos + 1..];
    }
    None
}
