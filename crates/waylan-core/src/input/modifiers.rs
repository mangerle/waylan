use super::constants::{
    KEY_LEFTALT, KEY_LEFTCTRL, KEY_LEFTMETA, KEY_LEFTSHIFT, KEY_RIGHTALT, KEY_RIGHTCTRL,
    KEY_RIGHTMETA, KEY_RIGHTSHIFT, MOD_ALT, MOD_CAPS, MOD_CTRL, MOD_SHIFT, MOD_SUPER,
};
use crate::state::SharedInputFlags;
use crate::InputMode;

/// 从聚合共享输入状态容器同步输入语言模式
///
/// 若模式发生变更，返回 `Some(new_mode)`；若未变更，返回 `None`。
pub fn sync_mode_from_shared_flags(
    shared_flags: Option<&SharedInputFlags>,
    current_mode: &mut InputMode,
) -> Option<InputMode> {
    if let Some(flags) = shared_flags {
        let external_zh = flags.is_chinese();
        let current_zh = matches!(*current_mode, InputMode::Chinese | InputMode::VMode);
        if external_zh != current_zh {
            let new_mode = if external_zh {
                InputMode::Chinese
            } else {
                InputMode::English
            };
            *current_mode = new_mode;
            return Some(new_mode);
        }
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

/// 共享修饰键状态跟踪器
#[derive(Debug, Default, Clone)]
pub struct ModifierState {
    pub depressed: u32,
    pub phys_mods: u8,
    pub locked: u32,
}

impl ModifierState {
    /// 依据物理按键按下/释放事件更新修饰键状态位掩码
    pub fn update(&mut self, keycode: u32, is_press: bool) {
        update_modifier_masks(&mut self.depressed, &mut self.phys_mods, keycode, is_press);
    }

    /// 将外部协议层（如 IBus X11 state 掩码）的修饰键状态与内部物理按键状态对齐
    ///
    /// 以内部 `phys_mods` 追踪的物理按键为下界，叠加外部协议层传入的修饰键掩码，
    /// 避免在未收到前导物理修饰键事件时丢失组合键，也避免单次外部掩码在后续按键中残留。
    pub fn sync_external_depressed(&mut self, external_depressed: u32) {
        let mut phys_depressed = 0u32;
        if (self.phys_mods & 0b0000_0011) != 0 {
            phys_depressed |= MOD_SHIFT;
        }
        if (self.phys_mods & 0b0000_1100) != 0 {
            phys_depressed |= MOD_CTRL;
        }
        if (self.phys_mods & 0b0011_0000) != 0 {
            phys_depressed |= MOD_ALT;
        }
        if (self.phys_mods & 0b1100_0000) != 0 {
            phys_depressed |= MOD_SUPER;
        }
        self.depressed = phys_depressed | external_depressed;
    }

    /// 查询 Shift 是否处于按下状态（左右任一侧均计入）
    #[inline]
    pub fn is_shift_active(&self) -> bool {
        (self.depressed & MOD_SHIFT) != 0
    }

    /// 查询 Ctrl 是否处于按下状态（左右任一侧均计入）
    #[inline]
    pub fn is_ctrl_active(&self) -> bool {
        (self.depressed & MOD_CTRL) != 0
    }

    /// 查询 Alt 是否处于按下状态（左右任一侧均计入）
    #[inline]
    pub fn is_alt_active(&self) -> bool {
        (self.depressed & MOD_ALT) != 0
    }

    /// 查询 CapsLock 是否处于锁定状态
    #[inline]
    pub fn is_caps_locked(&self) -> bool {
        (self.locked & MOD_CAPS) != 0
    }

    /// 查询是否按下 Ctrl / Alt / Super 任一控制键（用于判定组合键直通）
    #[inline]
    pub fn has_ctrl_or_alt(&self) -> bool {
        (self.depressed & (MOD_CTRL | MOD_ALT | MOD_SUPER)) != 0
    }

    /// 清空全部修饰键状态（用于会话激活、失焦或运行时重置）
    #[inline]
    pub fn reset(&mut self) {
        self.depressed = 0;
        self.phys_mods = 0;
        self.locked = 0;
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
    /// 创建空的按键消费追踪器
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

    /// 查询指定按键当前是否处于被消费按下状态（不清除状态）
    #[inline]
    pub fn is_consumed(&self, keycode: u32) -> bool {
        let idx = (keycode / 64) as usize;
        let bit = keycode % 64;
        if let Some(word) = self.bits.get(idx) {
            (*word & (1u64 << bit)) != 0
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
