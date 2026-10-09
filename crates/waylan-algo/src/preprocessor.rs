use waylan_core::{InputMode, TextAction};

/// Shift 键中英文切换防误触状态机
///
/// # 设计原理
/// - **实现初衷**：解决用户在快速打字时“单独敲击 Shift 切换中英文”与“按住 Shift 输入大写字母/符号”的冲突。
/// - **核心优势**：严格跟踪 Shift 按下期间是否伴随其他键，若伴随其他按键则判定为组合键输入，禁止切换模式。
/// - **代价与局限**：需外部键盘事件驱动严格传入按下与松开（Press/Release）成对事件。
#[derive(Debug, Default)]
pub struct ShiftToggleStateMachine {
    /// 记录 Shift 是否处于按下状态
    shift_pressed: bool,
    /// 记录在 Shift 按下期间是否伴随敲击了其他键
    other_key_pressed: bool,
}

impl ShiftToggleStateMachine {
    pub const KEY_LEFTSHIFT: u32 = 42;
    pub const KEY_RIGHTSHIFT: u32 = 54;

    pub fn new() -> Self {
        Self::default()
    }

    /// 接收原始按键扫描码事件并判定是否应当翻转中英文模式
    pub fn on_key_event(
        &mut self,
        keycode: u32,
        is_press: bool,
        current_mode: InputMode,
    ) -> Option<InputMode> {
        let is_shift = keycode == Self::KEY_LEFTSHIFT || keycode == Self::KEY_RIGHTSHIFT;

        if is_shift {
            if is_press {
                self.shift_pressed = true;
                self.other_key_pressed = false;
            } else {
                let should_toggle = self.shift_pressed && !self.other_key_pressed;
                self.shift_pressed = false;
                self.other_key_pressed = false;

                if should_toggle {
                    return Some(match current_mode {
                        InputMode::Chinese => InputMode::English,
                        InputMode::English => InputMode::Chinese,
                        InputMode::VMode => InputMode::Chinese,
                    });
                }
            }
        } else if self.shift_pressed {
            // 在 Shift 保持按下期间按下了其它键，判定为大写输入或组合热键
            self.other_key_pressed = true;
        }

        None
    }
}

/// 智能标点符号处理器
///
/// # 设计原理
/// - **实现初衷**：提供微信输入法式的舒适细节（斜杠自动转顿号、成对括号自动闭合左移光标、数字间冒号小数点保护）。
/// - **核心优势**：在拼音引擎外层形成纯函数式拦截，打数字（如 12:30 或 3.14）时无需切英文模式。
/// - **代价与局限**：需由外部在选词上屏时调用 `record_commit` 维护前驱字符上下文。
#[derive(Debug, Default)]
pub struct PunctuationProcessor {
    /// 记录上一次提交的字符是否是数字
    last_char_was_digit: bool,
}

impl PunctuationProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录外部提交动作以更新上下文状态
    pub fn record_commit(&mut self, text: &str) {
        if let Some(last_ch) = text.chars().last() {
            self.last_char_was_digit = last_ch.is_ascii_digit();
        }
    }

    /// 尝试处理标点符号输入
    pub fn process_punctuation(&mut self, ch: char) -> Option<Vec<TextAction>> {
        // 1. 数字间符号保护（例如 12:00 或 3.14，保持英文半角符号）
        if self.last_char_was_digit && (ch == ':' || ch == '.') {
            return Some(vec![TextAction::Commit(ch.to_string())]);
        }

        self.last_char_was_digit = false;

        // 2. 成对符号自动闭合并光标回退
        match ch {
            '(' => {
                return Some(vec![
                    TextAction::Commit("（）".to_string()),
                    TextAction::MoveCursor(-1),
                ]);
            }
            '<' => {
                return Some(vec![
                    TextAction::Commit("《》".to_string()),
                    TextAction::MoveCursor(-1),
                ]);
            }
            '[' => {
                return Some(vec![
                    TextAction::Commit("【】".to_string()),
                    TextAction::MoveCursor(-1),
                ]);
            }
            '{' => {
                return Some(vec![
                    TextAction::Commit("｛｝".to_string()),
                    TextAction::MoveCursor(-1),
                ]);
            }
            '"' => {
                return Some(vec![
                    TextAction::Commit("“”".to_string()),
                    TextAction::MoveCursor(-1),
                ]);
            }
            // 单引号（在独立标点输入模式下）
            '\'' => {
                return Some(vec![
                    TextAction::Commit("‘’".to_string()),
                    TextAction::MoveCursor(-1),
                ]);
            }
            _ => {}
        }

        // 3. 常规中文标点全角映射
        match ch {
            ',' => Some(vec![TextAction::Commit("，".to_string())]),
            '.' => Some(vec![TextAction::Commit("。".to_string())]),
            '/' => Some(vec![TextAction::Commit("、".to_string())]),
            '?' => Some(vec![TextAction::Commit("？".to_string())]),
            '!' => Some(vec![TextAction::Commit("！".to_string())]),
            ':' => Some(vec![TextAction::Commit("：".to_string())]),
            ';' => Some(vec![TextAction::Commit("；".to_string())]),
            '~' => Some(vec![TextAction::Commit("～".to_string())]),
            '$' => Some(vec![TextAction::Commit("￥".to_string())]),
            '^' => Some(vec![TextAction::Commit("……".to_string())]),
            '_' => Some(vec![TextAction::Commit("——".to_string())]),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shift_toggle() {
        let mut sm = ShiftToggleStateMachine::new();
        // 单击 Shift 切换模式
        assert_eq!(
            sm.on_key_event(
                ShiftToggleStateMachine::KEY_LEFTSHIFT,
                true,
                InputMode::Chinese
            ),
            None
        );
        assert_eq!(
            sm.on_key_event(
                ShiftToggleStateMachine::KEY_LEFTSHIFT,
                false,
                InputMode::Chinese
            ),
            Some(InputMode::English)
        );

        // Shift + A 不切换
        assert_eq!(
            sm.on_key_event(
                ShiftToggleStateMachine::KEY_LEFTSHIFT,
                true,
                InputMode::Chinese
            ),
            None
        );
        assert_eq!(sm.on_key_event(30, true, InputMode::Chinese), None); // A
        assert_eq!(
            sm.on_key_event(
                ShiftToggleStateMachine::KEY_LEFTSHIFT,
                false,
                InputMode::Chinese
            ),
            None
        );
    }

    #[test]
    fn test_punctuation_conversion() {
        let mut pp = PunctuationProcessor::new();
        // 斜杠替换顿号
        let res = pp.process_punctuation('/').expect("应替换顿号");
        assert_eq!(res, vec![TextAction::Commit("、".to_string())]);

        // 数字后冒号保护
        pp.record_commit("12");
        let res_colon = pp.process_punctuation(':').expect("应半角冒号");
        assert_eq!(res_colon, vec![TextAction::Commit(":".to_string())]);

        // 自动成对括号
        let res_bracket = pp.process_punctuation('(').expect("成对括号");
        assert_eq!(
            res_bracket,
            vec![
                TextAction::Commit("（）".to_string()),
                TextAction::MoveCursor(-1),
            ]
        );

        // 常规逗号句号映射
        assert_eq!(
            pp.process_punctuation(','),
            Some(vec![TextAction::Commit("，".to_string())])
        );
        assert_eq!(
            pp.process_punctuation('.'),
            Some(vec![TextAction::Commit("。".to_string())])
        );

        // 成对书名号《》
        let res_book = pp.process_punctuation('<').expect("成对书名号");
        assert_eq!(
            res_book,
            vec![
                TextAction::Commit("《》".to_string()),
                TextAction::MoveCursor(-1),
            ]
        );

        // 成对双引号“”
        let res_quote = pp.process_punctuation('"').expect("成对双引号");
        assert_eq!(
            res_quote,
            vec![
                TextAction::Commit("“”".to_string()),
                TextAction::MoveCursor(-1),
            ]
        );

        // 数字后小数点保护 3.14
        pp.record_commit("3");
        assert_eq!(
            pp.process_punctuation('.'),
            Some(vec![TextAction::Commit(".".to_string())])
        );
    }
}
