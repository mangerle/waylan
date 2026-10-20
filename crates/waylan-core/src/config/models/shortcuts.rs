/// 键盘修饰键（Ctrl / Alt / Shift / Super）的实时按下状态
///
/// # 设计原理
/// - **实现初衷**：将四个彼此独立、可同时按下的修饰键布尔量收敛为单一值类型，
///   避免快捷键匹配与展示格式化接口退化为平铺布尔参数列表。
/// - **核心优势**：调用点语义自解释，从类型层面杜绝实参错位
///   （例如把 Ctrl 状态误填进 Alt 位），同时天然支持整体相等比较。
/// - **代价与局限**：仅描述修饰键瞬时状态，不含主键信息，主键需另行传递。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers {
    /// Ctrl 键是否处于按下状态
    pub ctrl: bool,
    /// Alt 键是否处于按下状态
    pub alt: bool,
    /// Shift 键是否处于按下状态
    pub shift: bool,
    /// Super / Win 键是否处于按下状态
    pub super_key: bool,
}

impl KeyModifiers {
    /// 构造修饰键按下状态
    pub fn new(ctrl: bool, alt: bool, shift: bool, super_key: bool) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            super_key,
        }
    }
}

/// 归一化解析后的快捷键数据
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParsedShortcut {
    /// 主键所需搭配的修饰键状态
    pub modifiers: KeyModifiers,
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

        let mut modifiers = KeyModifiers::default();
        let mut evdev_key = None;

        for part in trimmed
            .split('+')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
        {
            let lower = part.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" | "mod1" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "super" | "win" | "meta" => modifiers.super_key = true,
                _ => {
                    if let Some(code) = parse_key_name(&lower) {
                        evdev_key = Some(code);
                    }
                }
            }
        }

        evdev_key.map(|k| Self {
            modifiers,
            evdev_key: k,
        })
    }

    /// 校验当前输入事件是否精确匹配该快捷键
    ///
    /// 主键扫描码与四个修饰键状态必须同时一致方视为命中。
    pub fn matches(&self, evdev_key: u32, modifiers: &KeyModifiers) -> bool {
        self.evdev_key == evdev_key && self.modifiers == *modifiers
    }
}

/// 标准常见按键小写名称与 Linux evdev 扫描码静态映射表
const STATIC_KEY_MAP: &[(&str, u32)] = &[
    ("space", 57),
    ("空格", 57),
    ("enter", 28),
    ("return", 28),
    ("回车", 28),
    ("tab", 15),
    ("esc", 1),
    ("escape", 1),
    ("backspace", 14),
    ("f1", 59),
    ("f2", 60),
    ("f3", 61),
    ("f4", 62),
    ("f5", 63),
    ("f6", 64),
    ("f7", 65),
    ("f8", 66),
    ("f9", 67),
    ("f10", 68),
    ("f11", 87),
    ("f12", 88),
    ("a", 30),
    ("b", 48),
    ("c", 46),
    ("d", 32),
    ("e", 18),
    ("f", 33),
    ("g", 34),
    ("h", 35),
    ("i", 23),
    ("j", 36),
    ("k", 37),
    ("l", 38),
    ("m", 50),
    ("n", 49),
    ("o", 24),
    ("p", 25),
    ("q", 16),
    ("r", 19),
    ("s", 31),
    ("t", 20),
    ("u", 22),
    ("v", 47),
    ("w", 17),
    ("x", 45),
    ("y", 21),
    ("z", 44),
    ("1", 2),
    ("2", 3),
    ("3", 4),
    ("4", 5),
    ("5", 6),
    ("6", 7),
    ("7", 8),
    ("8", 9),
    ("9", 10),
    ("0", 11),
    ("grave", 41),
    ("`", 41),
    ("minus", 12),
    ("-", 12),
    ("equal", 13),
    ("=", 13),
    ("comma", 51),
    (",", 51),
    ("period", 52),
    ("dot", 52),
    (".", 52),
    ("句点", 52),
    ("slash", 53),
    ("/", 53),
    ("semicolon", 39),
    (";", 39),
    ("[", 26),
    ("leftbracket", 26),
    ("]", 27),
    ("rightbracket", 27),
    ("\\", 43),
    ("backslash", 43),
    ("'", 40),
    ("apostrophe", 40),
    ("pageup", 104),
    ("pagedown", 109),
    ("up", 103),
    ("down", 108),
    ("left", 105),
    ("right", 106),
    ("delete", 111),
];

/// 将常见按键小写名称映射为标准 Linux evdev 扫描码
fn parse_key_name(name: &str) -> Option<u32> {
    STATIC_KEY_MAP
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, code)| *code)
}

/// 主键文本到规范展示名的静态映射表
///
/// 覆盖具名功能键（Space / Enter / Tab / 方向键等）与标点符号主键。
const KEY_DISPLAY_MAP: &[(&str, &str)] = &[
    ("space", "Space"),
    ("Space", "Space"),
    ("Return", "Enter"),
    ("Enter", "Enter"),
    ("Tab", "Tab"),
    ("PageUp", "PageUp"),
    ("pageup", "PageUp"),
    ("PageDown", "PageDown"),
    ("pagedown", "PageDown"),
    ("Up", "Up"),
    ("up", "Up"),
    ("UpArrow", "Up"),
    ("Down", "Down"),
    ("down", "Down"),
    ("DownArrow", "Down"),
    ("Left", "Left"),
    ("left", "Left"),
    ("LeftArrow", "Left"),
    ("Right", "Right"),
    ("right", "Right"),
    ("RightArrow", "Right"),
    ("句点", "."),
    ("dot", "."),
    ("period", "."),
    (".", "."),
    ("comma", ","),
    (",", ","),
    ("minus", "-"),
    ("-", "-"),
    ("equal", "="),
    ("=", "="),
    ("slash", "/"),
    ("/", "/"),
    ("semicolon", ";"),
    (";", ";"),
    ("grave", "`"),
    ("`", "`"),
    ("leftbracket", "["),
    ("[", "["),
    ("rightbracket", "]"),
    ("]", "]"),
    ("backslash", "\\"),
    ("\\", "\\"),
    ("apostrophe", "'"),
    ("'", "'"),
];

/// 将键盘组合键格式化为规范的展示文本（如 "Ctrl + Shift + F"、"Shift + Space"、"Ctrl + ."）
///
/// 若未按下有效主键或仅按下修饰键，则返回 `None`。
pub fn format_key_combination(modifiers: &KeyModifiers, text: &str) -> Option<String> {
    let key_display = resolve_key_display(text)?;
    Some(format_with_modifiers(modifiers, &key_display))
}

/// 解析主键文本为规范展示名，无法识别时返回 `None`
fn resolve_key_display(text: &str) -> Option<String> {
    // 空白与控制字符必须在裁剪前判定，否则会被 trim 吞掉而永远无法命中
    match text {
        " " => return Some("Space".to_string()),
        "\n" | "\r" => return Some("Enter".to_string()),
        "\t" => return Some("Tab".to_string()),
        _ => {}
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(num) = strip_function_key_suffix(trimmed) {
        return Some(format!("F{num}"));
    }
    if let Some(display) = KEY_DISPLAY_MAP
        .iter()
        .find(|(name, _)| *name == trimmed)
        .map(|(_, display)| *display)
    {
        return Some(display.to_string());
    }

    // 单字符字母统一以大写展示，单字符数字原样保留
    let mut chars = trimmed.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase().to_string()),
        (Some(c), None) if c.is_ascii_digit() => Some(c.to_string()),
        _ => None,
    }
}

/// 解析功能键后缀数字（如 "F8" 中的 "8"），非功能键返回 `None`
fn strip_function_key_suffix(trimmed: &str) -> Option<&str> {
    let num = trimmed.strip_prefix(['F', 'f'])?;
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(num)
}

/// 按 Ctrl、Alt、Shift、Super 的固定顺序拼接主键展示文本
fn format_with_modifiers(modifiers: &KeyModifiers, key: &str) -> String {
    let mut parts = Vec::with_capacity(5);
    if modifiers.ctrl {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    if modifiers.super_key {
        parts.push("Super");
    }
    parts.push(key);
    parts.join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsed_shortcut_parsing() {
        let p1 = ParsedShortcut::parse("Ctrl + Alt + V").expect("必须成功解析");
        assert!(p1.modifiers.ctrl);
        assert!(p1.modifiers.alt);
        assert!(!p1.modifiers.shift);
        assert!(!p1.modifiers.super_key);
        assert_eq!(p1.evdev_key, 47);

        let p2 = ParsedShortcut::parse("F8").expect("单键必须成功解析");
        assert!(!p2.modifiers.ctrl);
        assert!(!p2.modifiers.alt);
        assert_eq!(p2.evdev_key, 66);

        let p3 = ParsedShortcut::parse("Super + Shift + V").expect("Super键解析");
        assert!(p3.modifiers.super_key);
        assert!(p3.modifiers.shift);
        assert_eq!(p3.evdev_key, 47);

        assert!(ParsedShortcut::parse("已禁用").is_none());
        assert!(ParsedShortcut::parse("   ").is_none());

        let p4 = ParsedShortcut::parse("Ctrl + .").expect("标点快捷键解析");
        assert!(p4.modifiers.ctrl);
        assert_eq!(p4.evdev_key, 52);

        let p5 = ParsedShortcut::parse("Shift + Space").expect("空格快捷键解析");
        assert!(p5.modifiers.shift);
        assert_eq!(p5.evdev_key, 57);
    }

    #[test]
    fn test_format_key_combination() {
        let ctrl_shift = KeyModifiers::new(true, false, true, false);
        let shift_only = KeyModifiers::new(false, false, true, false);
        let ctrl_only = KeyModifiers::new(true, false, false, false);
        let super_only = KeyModifiers::new(false, false, false, true);
        let no_modifier = KeyModifiers::default();

        assert_eq!(
            format_key_combination(&ctrl_shift, "f").as_deref(),
            Some("Ctrl + Shift + F")
        );
        assert_eq!(
            format_key_combination(&shift_only, " ").as_deref(),
            Some("Shift + Space")
        );
        assert_eq!(
            format_key_combination(&ctrl_only, ".").as_deref(),
            Some("Ctrl + .")
        );
        assert_eq!(
            format_key_combination(&super_only, "v").as_deref(),
            Some("Super + V")
        );
        assert_eq!(
            format_key_combination(&no_modifier, "F8").as_deref(),
            Some("F8")
        );
        assert!(format_key_combination(&ctrl_only, "").is_none());
        assert!(format_key_combination(&ctrl_only, "\u{0001}").is_none());
    }

    #[test]
    fn test_format_key_combination_control_keys() {
        let no_modifier = KeyModifiers::default();

        assert_eq!(
            format_key_combination(&no_modifier, "\n").as_deref(),
            Some("Enter")
        );
        assert_eq!(
            format_key_combination(&no_modifier, "\t").as_deref(),
            Some("Tab")
        );
        assert_eq!(
            format_key_combination(&no_modifier, "Return").as_deref(),
            Some("Enter")
        );
        assert_eq!(
            format_key_combination(&no_modifier, "UpArrow").as_deref(),
            Some("Up")
        );
    }

    #[test]
    fn test_parsed_shortcut_matches_with_modifier_state() {
        let shortcut = ParsedShortcut::parse("Ctrl + Shift + V").expect("必须成功解析");
        assert!(shortcut.matches(47, &KeyModifiers::new(true, false, true, false)));
        assert!(!shortcut.matches(47, &KeyModifiers::new(true, false, false, false)));
        assert!(!shortcut.matches(48, &KeyModifiers::new(true, false, true, false)));
    }
}
