//! 核心输入决策与按键处理模型
//!
//! 包含 Linux 扫描码常量、翻页配置、扫描码转换、物理修饰键位图跟踪与动态键盘布局映射。

pub mod constants;
pub mod conversion;
pub mod keymap;
pub mod modifiers;
pub mod page_keys;
pub mod punctuation;
pub mod shift_toggle;

pub use constants::*;
pub use conversion::*;
pub use keymap::KeymapLayout;
pub use modifiers::*;
pub use page_keys::*;
pub use punctuation::PunctuationProcessor;
pub use shift_toggle::{ShiftToggleStateMachine, ToggleKeyMode};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consumed_key_tracker_symmetry() {
        let mut tracker = ConsumedKeyTracker::new();
        let key = KEY_SPACE;

        // 默认未消费
        assert!(!tracker.is_consumed(key));

        // 记录按下被消费
        tracker.record_press(key, true);
        assert!(tracker.is_consumed(key));

        // 释放按键提取决策，之后状态复位
        assert!(tracker.take_release(key));
        assert!(!tracker.is_consumed(key));
        assert!(!tracker.take_release(key));
    }

    #[test]
    fn test_keycode_conversions() {
        assert_eq!(keycode_to_char(KEY_1), None);
        assert_eq!(keycode_to_digit(KEY_1), Some(1));
        assert_eq!(keycode_to_digit(KEY_0), Some(10));
        assert_eq!(keycode_to_ascii_digit_char(KEY_1), Some('1'));
        assert_eq!(keycode_to_ascii_digit_char(KEY_KP5), Some('5'));
        assert_eq!(keycode_to_punctuation(KEY_COMMA), Some(','));
        assert_eq!(
            keycode_to_punctuation_with_shift(KEY_COMMA, true),
            Some('<')
        );
    }

    #[test]
    fn test_page_key_config() {
        let cfg = PageKeyConfig {
            minus_equal: true,
            bracket: false,
            comma_period: false,
        };
        assert!(cfg.is_prev_page(KEY_MINUS));
        assert!(cfg.is_next_page(KEY_EQUAL));
        assert!(!cfg.is_prev_page(KEY_COMMA));
        assert!(cfg.is_prev_page(KEY_PAGEUP));
        assert!(cfg.is_next_page(KEY_PAGEDOWN));
    }

    #[test]
    fn test_modifier_state() {
        let mut depressed = 0u32;
        let mut phys_mods = 0u8;

        // 左 Shift 按下
        update_modifier_masks(&mut depressed, &mut phys_mods, KEY_LEFTSHIFT, true);
        assert_ne!(depressed & MOD_SHIFT, 0);
        assert_eq!(phys_mods, 0b0000_0001);

        // 右 Shift 按下
        update_modifier_masks(&mut depressed, &mut phys_mods, KEY_RIGHTSHIFT, true);
        assert_ne!(depressed & MOD_SHIFT, 0);
        assert_eq!(phys_mods, 0b0000_0011);

        // 左 Shift 松开，但右 Shift 仍按着，MOD_SHIFT 依然置位
        update_modifier_masks(&mut depressed, &mut phys_mods, KEY_LEFTSHIFT, false);
        assert_ne!(depressed & MOD_SHIFT, 0);
        assert_eq!(phys_mods, 0b0000_0010);

        // 右 Shift 松开，MOD_SHIFT 完全清除
        update_modifier_masks(&mut depressed, &mut phys_mods, KEY_RIGHTSHIFT, false);
        assert_eq!(depressed & MOD_SHIFT, 0);
        assert_eq!(phys_mods, 0);
    }
}
