/// 归一化解析后的快捷键数据
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParsedShortcut {
    /// 是否需要按住 Ctrl
    pub ctrl: bool,
    /// 是否需要按住 Alt
    pub alt: bool,
    /// 是否需要按住 Shift
    pub shift: bool,
    /// 是否需要按住 Super / Win
    pub super_key: bool,
    /// 目标主键的 Linux evdev 扫描码
    pub evdev_key: u32,
}

impl ParsedShortcut {
    /// 从用户输入的快捷键描述文本解析（如 "Ctrl+Alt+V", "Alt+Space", "F8", "Super+V" 等）
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("disabled") || trimmed == "已禁用"
        {
            return None;
        }

        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut super_key = false;
        let mut evdev_key = None;

        for part in trimmed
            .split('+')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
        {
            let lower = part.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "mod1" => alt = true,
                "shift" => shift = true,
                "super" | "win" | "meta" => super_key = true,
                _ => {
                    if let Some(code) = parse_key_name(&lower) {
                        evdev_key = Some(code);
                    }
                }
            }
        }

        evdev_key.map(|k| Self {
            ctrl,
            alt,
            shift,
            super_key,
            evdev_key: k,
        })
    }

    /// 校验当前输入事件是否精确匹配该快捷键
    pub fn matches(
        &self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
        super_active: bool,
    ) -> bool {
        self.evdev_key == evdev_key
            && self.ctrl == ctrl_active
            && self.alt == alt_active
            && self.shift == shift_active
            && self.super_key == super_active
    }
}

/// 将常见按键小写名称映射为标准 Linux evdev 扫描码
fn parse_key_name(name: &str) -> Option<u32> {
    let code = match name {
        "space" | "空格" => 57,
        "enter" | "return" | "回车" => 28,
        "tab" => 15,
        "esc" | "escape" => 1,
        "backspace" => 14,
        "f1" => 59,
        "f2" => 60,
        "f3" => 61,
        "f4" => 62,
        "f5" => 63,
        "f6" => 64,
        "f7" => 65,
        "f8" => 66,
        "f9" => 67,
        "f10" => 68,
        "f11" => 87,
        "f12" => 88,
        "a" => 30,
        "b" => 48,
        "c" => 46,
        "d" => 32,
        "e" => 18,
        "f" => 33,
        "g" => 34,
        "h" => 35,
        "i" => 23,
        "j" => 36,
        "k" => 37,
        "l" => 38,
        "m" => 50,
        "n" => 49,
        "o" => 24,
        "p" => 25,
        "q" => 16,
        "r" => 19,
        "s" => 31,
        "t" => 20,
        "u" => 22,
        "v" => 47,
        "w" => 17,
        "x" => 45,
        "y" => 21,
        "z" => 44,
        "1" => 2,
        "2" => 3,
        "3" => 4,
        "4" => 5,
        "5" => 6,
        "6" => 7,
        "7" => 8,
        "8" => 9,
        "9" => 10,
        "0" => 11,
        "grave" | "`" => 41,
        "minus" | "-" => 12,
        "equal" | "=" => 13,
        "comma" | "," => 51,
        "period" | "dot" | "." => 52,
        "slash" | "/" => 53,
        "semicolon" | ";" => 39,
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsed_shortcut_parsing() {
        let p1 = ParsedShortcut::parse("Ctrl + Alt + V").expect("必须成功解析");
        assert!(p1.ctrl);
        assert!(p1.alt);
        assert!(!p1.shift);
        assert!(!p1.super_key);
        assert_eq!(p1.evdev_key, 47);

        let p2 = ParsedShortcut::parse("F8").expect("单键必须成功解析");
        assert!(!p2.ctrl);
        assert!(!p2.alt);
        assert_eq!(p2.evdev_key, 66);

        let p3 = ParsedShortcut::parse("Super + Shift + V").expect("Super键解析");
        assert!(p3.super_key);
        assert!(p3.shift);
        assert_eq!(p3.evdev_key, 47);

        assert!(ParsedShortcut::parse("已禁用").is_none());
        assert!(ParsedShortcut::parse("   ").is_none());
    }
}
