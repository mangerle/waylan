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

/// 将 Linux 扫描码转换为 ASCII 小写英文字母
pub fn keycode_to_char(keycode: u32) -> Option<char> {
    match keycode {
        30 => Some('a'),
        48 => Some('b'),
        46 => Some('c'),
        32 => Some('d'),
        18 => Some('e'),
        33 => Some('f'),
        34 => Some('g'),
        35 => Some('h'),
        23 => Some('i'),
        36 => Some('j'),
        37 => Some('k'),
        38 => Some('l'),
        50 => Some('m'),
        49 => Some('n'),
        24 => Some('o'),
        25 => Some('p'),
        16 => Some('q'),
        19 => Some('r'),
        31 => Some('s'),
        20 => Some('t'),
        22 => Some('u'),
        47 => Some('v'),
        17 => Some('w'),
        45 => Some('x'),
        21 => Some('y'),
        44 => Some('z'),
        _ => None,
    }
}

/// 将扫描码转换为 1-indexed 选词序号
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
#[derive(Debug, PartialEq, Eq)]
pub enum KeyProcessAction {
    /// 原样透传给客户端应用
    PassThrough,
    /// 内部消费，刷新候选表面（无需提交）
    Handled,
    /// 提交整段文字上屏，并隐藏候选表面
    Commit(String),
    /// 提交部分文字，并保留/刷新剩余拼音预编辑
    CommitPartial {
        committed: String,
        remaining_preedit: String,
    },
    /// 清空预编辑并重置状态
    Reset,
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
/// - **核心优势**：采用 `[u64; 4]` 栈上位图覆盖 0..256 全部标准键盘扫描码，查询与更新均为 $O(1)$ 位运算且无堆分配。
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsumedKeyTracker {
    bits: [u64; 4],
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
        self.bits = [0; 4];
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
        assert_eq!(keycode_to_digit(KEY_0), None);

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
}
