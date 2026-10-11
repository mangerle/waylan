/// Linux 常见按键键码常量（基于 linux/input-event-codes.h）
pub const KEY_ESC: u32 = 1;
pub const KEY_MINUS: u32 = 12;
pub const KEY_EQUAL: u32 = 13;
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_TAB: u32 = 15;
pub const KEY_ENTER: u32 = 28;
pub const KEY_SPACE: u32 = 57;
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

/// 标准 XKB 修饰键位掩码常量
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CTRL: u32 = 4;
pub const MOD_ALT: u32 = 8;
pub const MOD_SUPER: u32 = 64;

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

/// 键盘字符映射表（支持运行时动态从 XKB Keymap 文本更新，适配 Dvorak、Colemak、AZERTY 等非 QWERTY 布局）
#[derive(Debug, Clone)]
pub struct KeymapLayout {
    char_map: std::collections::HashMap<u32, char>,
}

impl Default for KeymapLayout {
    fn default() -> Self {
        Self::qwerty()
    }
}

impl KeymapLayout {
    /// 标准 QWERTY 布局默认映射
    pub fn qwerty() -> Self {
        let mut map = std::collections::HashMap::with_capacity(32);
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

    /// 解析 XKB 字符定义并更新字符映射表（支持常见 Linux/XKB 符号语法）
    pub fn update_from_xkb_text(&mut self, text: &str) {
        // 标准 evdev 按键名到 Linux 硬件扫描码的固定映射表
        let evdev_keynames: &[(&str, u32)] = &[
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
        ];

        let mut parsed_map = std::collections::HashMap::with_capacity(32);
        for &(keyname, code) in evdev_keynames {
            let pattern = format!("<{}>", keyname);
            if let Some(pos) = text.find(&pattern) {
                // 查找该按键定义块中首个符号列表: [ sym1, sym2 ]
                let slice = &text[pos..];
                if let Some(brace_end) = slice.find(';') {
                    let key_block = &slice[..brace_end];
                    if let Some(bracket_start) = key_block.find('[') {
                        if let Some(bracket_end) = key_block[bracket_start..].find(']') {
                            let syms = &key_block[bracket_start + 1..bracket_start + bracket_end];
                            // 提取第一个非空符号
                            if let Some(first_sym) = syms.split(',').next() {
                                let sym = first_sym.trim();
                                if sym.len() == 1 {
                                    let c = sym.chars().next().unwrap_or('\0').to_ascii_lowercase();
                                    if c.is_ascii_alphabetic() {
                                        parsed_map.insert(code, c);
                                    }
                                }
                            }
                        }
                    }
                }
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

/// 将 Linux 扫描码转换为 ASCII 小写英文字母（基于默认 QWERTY 布局）
pub fn keycode_to_char(keycode: u32) -> Option<char> {
    KeymapLayout::qwerty().get_char(keycode)
}

/// 根据指定键盘布局将 Linux 扫描码转换为 ASCII 小写英文字母
pub fn keycode_to_char_with_layout(keycode: u32, layout: &KeymapLayout) -> Option<char> {
    layout.get_char(keycode)
}

/// 将扫描码转换为 1-indexed 选词序号（1..=9 对应 1..=9 号候选，0 对应第 10 号候选）
pub fn keycode_to_digit(keycode: u32) -> Option<usize> {
    match keycode {
        KEY_1 => Some(1),
        KEY_2 => Some(2),
        KEY_3 => Some(3),
        KEY_4 => Some(4),
        KEY_5 => Some(5),
        KEY_6 => Some(6),
        KEY_7 => Some(7),
        KEY_8 => Some(8),
        KEY_9 => Some(9),
        KEY_0 => Some(10),
        _ => None,
    }
}

/// 将主键盘数字键扫描码转换为对应的 ASCII 数字字符 ('0'..='9')
pub fn keycode_to_ascii_digit_char(keycode: u32) -> Option<char> {
    match keycode {
        KEY_1 => Some('1'),
        KEY_2 => Some('2'),
        KEY_3 => Some('3'),
        KEY_4 => Some('4'),
        KEY_5 => Some('5'),
        KEY_6 => Some('6'),
        KEY_7 => Some('7'),
        KEY_8 => Some('8'),
        KEY_9 => Some('9'),
        KEY_0 => Some('0'),
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

/// 处理处于拼音组词状态（raw_input 非空）下的核心按键拦截决策
///
/// # 设计原理
/// - **实现初衷**：统一 V1 与 V2 协议在输入拼音时的按键分发逻辑（二级展开、候选词高亮移动、数字/空格选词、回车字母上屏、退格删除、翻页、光标移动与取消）。
/// - **核心优势**：直接操作 AlgoEngine，返回纯粹的 `KeyProcessAction` 决策枚举，调用方只需负责与自身协议的 IPC 消息映射。
pub fn handle_composition_key(
    engine: &mut waylan_algo::AlgoEngine,
    punct_processor: &mut waylan_algo::preprocessor::PunctuationProcessor,
    key: u32,
    shift_active: bool,
    page_keys: PageKeyConfig,
    now: u64,
) -> Option<KeyProcessAction> {
    // 1. Tab 键：切换/展开当前高亮候选词的二级动态格式卡片
    if key == KEY_TAB {
        let sel = engine.selected_index;
        engine.toggle_expand(sel);
        return Some(KeyProcessAction::Handled);
    }

    // 2. 处于展开二级状态下的导航与选择
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
        if key == KEY_ENTER || key == KEY_SPACE {
            if let Some((committed, _)) =
                engine.select_expanded_item(sel, engine.expanded_selected_index)
            {
                punct_processor.record_commit(&committed);
                return Some(KeyProcessAction::Commit(committed));
            }
        }
    } else {
        // 非展开态下，上下方向键在当前页候选词间移动高亮
        if key == KEY_DOWN {
            engine.next_candidate();
            return Some(KeyProcessAction::Handled);
        }
        if key == KEY_UP {
            engine.prev_candidate();
            return Some(KeyProcessAction::Handled);
        }
    }

    // 3. 数字键（1-9）：选取对应编号候选词
    if !shift_active {
        if let Some(digit) = keycode_to_digit(key) {
            let cand_idx = digit.saturating_sub(1);
            if let Some((committed_text, remaining_len)) = engine.select_candidate(cand_idx, now) {
                punct_processor.record_commit(&committed_text);
                if remaining_len > 0 {
                    return Some(KeyProcessAction::CommitPartial {
                        committed: committed_text,
                        remaining_preedit: engine.formatted_preedit(),
                    });
                } else {
                    return Some(KeyProcessAction::Commit(committed_text));
                }
            } else {
                // 组合态下超出范围的数字键吞掉，避免穿透给客户端引发字符污染
                return Some(KeyProcessAction::Handled);
            }
        }
    }

    // 4. 空格键：当前高亮候选词上屏（若无候选词则吞掉按键，避免空格泄露至宿主应用）
    if key == KEY_SPACE {
        let sel = engine.selected_index;
        if let Some((committed_text, remaining_len)) = engine.select_candidate(sel, now) {
            punct_processor.record_commit(&committed_text);
            if remaining_len > 0 {
                return Some(KeyProcessAction::CommitPartial {
                    committed: committed_text,
                    remaining_preedit: engine.formatted_preedit(),
                });
            } else {
                return Some(KeyProcessAction::Commit(committed_text));
            }
        } else {
            return Some(KeyProcessAction::Handled);
        }
    }

    // 5. 回车键（Enter）：直接上屏原始未转换的拼音英文字母
    if key == KEY_ENTER {
        let raw = engine.raw_input().to_string();
        engine.reset();
        return Some(KeyProcessAction::Commit(raw));
    }

    // 6. 退格键（Backspace）：删除一个拼文字母
    if key == KEY_BACKSPACE {
        engine.pop_char();
        if engine.raw_input().is_empty() {
            return Some(KeyProcessAction::Reset);
        } else {
            return Some(KeyProcessAction::Handled);
        }
    }

    // 7. 候选词翻页（根据配置的翻页快捷键模式判定）
    if !shift_active && page_keys.is_prev_page(key) {
        engine.prev_page();
        return Some(KeyProcessAction::Handled);
    }
    if !shift_active && page_keys.is_next_page(key) {
        engine.next_page();
        return Some(KeyProcessAction::Handled);
    }

    // 8. 预编辑区内光标左移/右移
    if key == KEY_LEFT {
        engine.move_cursor_left();
        return Some(KeyProcessAction::Handled);
    }
    if key == KEY_RIGHT {
        engine.move_cursor_right();
        return Some(KeyProcessAction::Handled);
    }

    // 9. Delete 键：删除光标处的字符
    if key == KEY_DELETE {
        engine.delete_char_at_cursor();
        if engine.raw_input().is_empty() {
            return Some(KeyProcessAction::Reset);
        } else {
            return Some(KeyProcessAction::Handled);
        }
    }

    // 10. ESC 键：取消当前输入并清空
    if key == KEY_ESC {
        engine.reset();
        return Some(KeyProcessAction::Reset);
    }

    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keycode_conversions() {
        assert_eq!(keycode_to_char(30), Some('a'));
        assert_eq!(keycode_to_char(44), Some('z'));
        assert_eq!(keycode_to_char(999), None);

        assert_eq!(keycode_to_digit(KEY_1), Some(1));
        assert_eq!(keycode_to_digit(KEY_9), Some(9));
        assert_eq!(keycode_to_digit(KEY_0), Some(10));
        assert_eq!(keycode_to_digit(KEY_ESC), None);

        assert_eq!(keycode_to_ascii_digit_char(KEY_0), Some('0'));
        assert_eq!(keycode_to_punctuation(KEY_COMMA), Some(','));
        assert_eq!(keycode_to_punctuation_with_shift(KEY_1, true), Some('!'));
    }

    #[test]
    fn test_page_key_config() {
        let cfg = PageKeyConfig::MinusEqual;
        assert!(cfg.is_prev_page(KEY_MINUS));
        assert!(cfg.is_next_page(KEY_EQUAL));
        assert!(cfg.is_prev_page(KEY_PAGEUP));
        assert!(cfg.is_next_page(KEY_PAGEDOWN));
    }

    #[test]
    fn test_modifier_state() {
        let mut mods = ModifierState::default();
        assert!(!mods.is_shift_active());
        assert!(!mods.has_ctrl_or_alt());

        // 同时按下左 Shift 与右 Shift，松开左 Shift 后仍应保持 Shift 激活
        mods.update(KEY_LEFTSHIFT, true);
        mods.update(KEY_RIGHTSHIFT, true);
        assert!(mods.is_shift_active());
        mods.update(KEY_LEFTSHIFT, false);
        assert!(
            mods.is_shift_active(),
            "松开单侧 Shift 时若另一侧仍按下，不应清除 MOD_SHIFT"
        );
        mods.update(KEY_RIGHTSHIFT, false);
        assert!(!mods.is_shift_active());

        // 测试 Super/Meta 键跟踪
        mods.update(KEY_LEFTMETA, true);
        assert!(mods.has_ctrl_or_alt());
        mods.update(KEY_LEFTMETA, false);
        assert!(!mods.has_ctrl_or_alt());

        mods.update(KEY_LEFTCTRL, true);
        assert!(mods.has_ctrl_or_alt());
        mods.reset();
        assert!(!mods.has_ctrl_or_alt());
    }

    #[test]
    fn test_consumed_key_tracker_symmetry() {
        let mut tracker = ConsumedKeyTracker::new();
        tracker.record_press(KEY_SPACE, true);
        tracker.record_press(KEY_LEFTSHIFT, false);

        assert!(tracker.take_release(KEY_SPACE));
        assert!(!tracker.take_release(KEY_SPACE));
        assert!(!tracker.take_release(KEY_LEFTSHIFT));
    }

    #[test]
    fn test_keymap_layout_dynamic_update() {
        let mut layout = KeymapLayout::qwerty();
        // 初始 QWERTY: 扫描码 30 映射到 'a'
        assert_eq!(layout.get_char(30), Some('a'));

        // 模拟一段 XKB symbols 文本（例如将 AC01 映射为 'o'）
        let mock_xkb = r#"
            key <AC01> { [ o, O ] };
            key <AC02> { [ a, A ] };
            key <AC03> { [ e, E ] };
            key <AC04> { [ u, U ] };
            key <AC05> { [ i, I ] };
            key <AC06> { [ d, D ] };
            key <AC07> { [ h, H ] };
            key <AC08> { [ t, T ] };
            key <AC09> { [ n, N ] };
            key <AC10> { [ s, S ] };
        "#;
        layout.update_from_xkb_text(mock_xkb);
        assert_eq!(layout.get_char(30), Some('o'));
        assert_eq!(layout.get_char(31), Some('a'));
        // 未在文本中重定义的按键保持 QWERTY 兜底
        assert_eq!(layout.get_char(48), Some('b'));
    }

    #[test]
    fn test_handle_composition_key() {
        let mut engine =
            waylan_algo::AlgoEngine::with_sample_data(waylan_algo::types::AlgoConfig::default());
        let mut punct = waylan_algo::preprocessor::PunctuationProcessor::new();
        let page_keys = PageKeyConfig::MinusEqual;

        // 模拟输入 "ni"
        engine.push_char('n');
        engine.push_char('i');
        assert_eq!(engine.raw_input(), "ni");

        // 测试光标移动
        let act = handle_composition_key(&mut engine, &mut punct, KEY_LEFT, false, page_keys, 0);
        assert_eq!(act, Some(KeyProcessAction::Handled));

        let act = handle_composition_key(&mut engine, &mut punct, KEY_RIGHT, false, page_keys, 0);
        assert_eq!(act, Some(KeyProcessAction::Handled));

        // 测试回车字母上屏
        let act = handle_composition_key(&mut engine, &mut punct, KEY_ENTER, false, page_keys, 0);
        assert_eq!(act, Some(KeyProcessAction::Commit("ni".to_string())));
        assert!(engine.raw_input().is_empty());

        // 测试 ESC 清空
        engine.push_char('a');
        let act = handle_composition_key(&mut engine, &mut punct, KEY_ESC, false, page_keys, 0);
        assert_eq!(act, Some(KeyProcessAction::Reset));
        assert!(engine.raw_input().is_empty());
    }
}
