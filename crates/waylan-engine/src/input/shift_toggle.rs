//! Shift/Ctrl 键中英文切换防误触状态机
//!
//! # 设计原理
//! - **实现初衷**：解决用户在快速打字时“单独敲击修饰键切换中英文”与“按住修饰键输入大写字母/组合快捷键”的冲突。
//! - **核心优势**：严格跟踪目标修饰键按下期间是否伴随其他键，若伴随其他按键则判定为组合键输入，禁止切换模式。
//! - **代价与局限**：需外部键盘事件驱动严格传入按下与松开（Press/Release）成对事件。

use waylan_core::config::SwitchKey;
use waylan_core::InputMode;

/// 中英文切换触发键模式（对齐全局统一配置项 [`SwitchKey`]）
pub type ToggleKeyMode = SwitchKey;

/// Shift/Ctrl 键中英文切换防误触状态机
#[derive(Debug, Default)]
pub struct ShiftToggleStateMachine {
    /// 当前配置的切换键模式（Shift 或 Ctrl）
    toggle_key: ToggleKeyMode,
    /// 记录切换修饰键是否处于按下状态
    modifier_pressed: bool,
    /// 记录在修饰键按下期间是否伴随敲击了其他键
    other_key_pressed: bool,
}

impl ShiftToggleStateMachine {
    pub const KEY_LEFTSHIFT: u32 = 42;
    pub const KEY_RIGHTSHIFT: u32 = 54;
    pub const KEY_LEFTCTRL: u32 = 29;
    pub const KEY_RIGHTCTRL: u32 = 97;

    pub fn new() -> Self {
        Self::default()
    }

    /// 使用指定的切换键模式创建状态机
    pub fn with_toggle_key(toggle_key: ToggleKeyMode) -> Self {
        Self {
            toggle_key,
            modifier_pressed: false,
            other_key_pressed: false,
        }
    }

    /// 获取当前生效的中英文切换键模式
    #[inline]
    pub fn toggle_key(&self) -> ToggleKeyMode {
        self.toggle_key
    }

    /// 重置切换修饰键按下与组合键记忆状态（用于焦点切换或会话失焦）
    pub fn reset(&mut self) {
        self.modifier_pressed = false;
        self.other_key_pressed = false;
    }

    /// 动态更新中英文切换键模式（若模式未发生改变则保留当前按键状态，避免热重载打断按键状态机）
    pub fn set_toggle_key(&mut self, toggle_key: ToggleKeyMode) {
        if self.toggle_key != toggle_key {
            self.toggle_key = toggle_key;
            self.reset();
        }
    }

    /// 接收原始按键扫描码事件并判定是否应当翻转中英文模式
    pub fn on_key_event(
        &mut self,
        keycode: u32,
        is_press: bool,
        current_mode: InputMode,
    ) -> Option<InputMode> {
        let is_target_mod = match self.toggle_key {
            ToggleKeyMode::Shift => {
                keycode == Self::KEY_LEFTSHIFT || keycode == Self::KEY_RIGHTSHIFT
            }
            ToggleKeyMode::Ctrl => keycode == Self::KEY_LEFTCTRL || keycode == Self::KEY_RIGHTCTRL,
        };

        if is_target_mod {
            if is_press {
                if !self.modifier_pressed {
                    self.modifier_pressed = true;
                    self.other_key_pressed = false;
                } else {
                    // 双侧同名修饰键交叠按下（如按住 LeftShift 时再按 RightShift），判定为组合键，不重置状态
                    self.other_key_pressed = true;
                }
            } else {
                let should_toggle = self.modifier_pressed && !self.other_key_pressed;
                self.modifier_pressed = false;
                self.other_key_pressed = false;

                if should_toggle {
                    return Some(match current_mode {
                        InputMode::Chinese => InputMode::English,
                        InputMode::English => InputMode::Chinese,
                        InputMode::VMode => InputMode::Chinese,
                    });
                }
            }
        } else if self.modifier_pressed {
            // 在目标修饰键保持按下期间收到任何非目标键的按下或释放事件，均判定为组合操作
            self.other_key_pressed = true;
        }

        None
    }
}
