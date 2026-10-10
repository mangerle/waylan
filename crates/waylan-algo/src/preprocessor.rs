use waylan_core::{InputMode, TextAction};

/// 中英文切换触发键模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToggleKeyMode {
    /// 左右 Shift 键切换（默认）
    #[default]
    Shift,
    /// 左右 Ctrl 键切换
    Ctrl,
}

/// Shift/Ctrl 键中英文切换防误触状态机
///
/// # 设计原理
/// - **实现初衷**：解决用户在快速打字时“单独敲击修饰键切换中英文”与“按住修饰键输入大写字母/组合快捷键”的冲突。
/// - **核心优势**：严格跟踪目标修饰键按下期间是否伴随其他键，若伴随其他按键则判定为组合键输入，禁止切换模式。
/// - **代价与局限**：需外部键盘事件驱动严格传入按下与松开（Press/Release）成对事件。
#[derive(Debug, Default)]
pub struct ShiftToggleStateMachine {
    /// 当前配置的切换键模式（Shift 或 Ctrl）
    pub toggle_key: ToggleKeyMode,
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
                self.modifier_pressed = true;
                self.other_key_pressed = false;
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
        } else if self.modifier_pressed && is_press {
            // 在目标修饰键保持按下期间按下了其它键，判定为大写输入或组合热键
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
#[derive(Debug, Clone)]
pub struct PunctuationProcessor {
    /// 记录上一次提交的字符是否是数字
    last_char_was_digit: bool,
    /// 双引号开闭状态位（false 为待输入左双引号，true 为已输入左双引号待输入右双引号）
    pub double_quote_open: bool,
    /// 单引号开闭状态位（false 为待输入左单引号，true 为已输入左单引号待输入右单引号）
    pub single_quote_open: bool,
    /// 是否将斜杠 / 转换为中文顿号 、
    pub slash_to_dunhao: bool,
    /// 是否开启数字后紧跟冒号/小数点/斜杠自动保持英文半角保护
    pub protect_digits_punctuation: bool,
    /// 是否开启成对括号与引号自动补全并回退光标
    pub auto_complete_pairs: bool,
}

impl Default for PunctuationProcessor {
    fn default() -> Self {
        Self {
            last_char_was_digit: false,
            double_quote_open: false,
            single_quote_open: false,
            slash_to_dunhao: true,
            protect_digits_punctuation: true,
            auto_complete_pairs: true,
        }
    }
}

impl PunctuationProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用指定的标点特性开关构造处理器
    pub fn with_options(
        slash_to_dunhao: bool,
        protect_digits_punctuation: bool,
        auto_complete_pairs: bool,
    ) -> Self {
        Self {
            last_char_was_digit: false,
            double_quote_open: false,
            single_quote_open: false,
            slash_to_dunhao,
            protect_digits_punctuation,
            auto_complete_pairs,
        }
    }

    /// 重置成对引号的开闭记忆状态
    pub fn reset_quote_states(&mut self) {
        self.double_quote_open = false;
        self.single_quote_open = false;
    }

    /// 记录外部提交动作以更新上下文状态（维护数字状态与成对引号开闭状态）
    pub fn record_commit(&mut self, text: &str) {
        if let Some(last_ch) = text.chars().last() {
            self.last_char_was_digit = last_ch.is_ascii_digit();
        }
        for ch in text.chars() {
            match ch {
                '“' => self.double_quote_open = true,
                '”' => self.double_quote_open = false,
                '‘' => self.single_quote_open = true,
                '’' => self.single_quote_open = false,
                _ => {}
            }
        }
    }

    /// 检查输入缓冲区是否属于网址模式或邮箱模式
    pub fn is_url_or_email_mode(buffer: &str) -> bool {
        let lower = buffer.to_lowercase();
        lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("ftp://")
            || lower.starts_with("www.")
            || lower.starts_with("git@")
            || lower.contains('@')
    }

    /// 尝试处理标点符号输入（兼容无缓冲区上下文接口）
    pub fn process_punctuation(&mut self, ch: char) -> Option<Vec<TextAction>> {
        self.process_punctuation_with_context(ch, "")
    }

    /// 结合当前输入缓冲区上下文处理标点符号输入
    pub fn process_punctuation_with_context(
        &mut self,
        ch: char,
        buffer: &str,
    ) -> Option<Vec<TextAction>> {
        // 1. 网址与邮箱模式感知：当前缓冲区为 URL 或 Email 时，常见网络标点强制保持半角 ASCII
        if Self::is_url_or_email_mode(buffer) && ".:/_?#&=@~-+".contains(ch) {
            return Some(vec![TextAction::Commit(ch.to_string())]);
        }

        // 2. 数字间/数字后符号保护（例如 12:00, 3.14, 2026/10/10，保持英文半角符号）
        if self.protect_digits_punctuation
            && self.last_char_was_digit
            && (ch == ':' || ch == '.' || ch == '/')
        {
            return Some(vec![TextAction::Commit(ch.to_string())]);
        }

        self.last_char_was_digit = false;

        // 3. 成对符号处理（开启自动配对时闭合双符号并回退光标，关闭时仅输出单侧中文符号）
        match ch {
            '(' => {
                if self.auto_complete_pairs {
                    return Some(vec![
                        TextAction::Commit("（）".to_string()),
                        TextAction::MoveCursor(-1),
                    ]);
                } else {
                    return Some(vec![TextAction::Commit("（".to_string())]);
                }
            }
            ')' => {
                return Some(vec![TextAction::Commit("）".to_string())]);
            }
            '<' => {
                if self.auto_complete_pairs {
                    return Some(vec![
                        TextAction::Commit("《》".to_string()),
                        TextAction::MoveCursor(-1),
                    ]);
                } else {
                    return Some(vec![TextAction::Commit("《".to_string())]);
                }
            }
            '>' => {
                return Some(vec![TextAction::Commit("》".to_string())]);
            }
            '[' => {
                if self.auto_complete_pairs {
                    return Some(vec![
                        TextAction::Commit("【】".to_string()),
                        TextAction::MoveCursor(-1),
                    ]);
                } else {
                    return Some(vec![TextAction::Commit("【".to_string())]);
                }
            }
            ']' => {
                return Some(vec![TextAction::Commit("】".to_string())]);
            }
            '{' => {
                if self.auto_complete_pairs {
                    return Some(vec![
                        TextAction::Commit("｛｝".to_string()),
                        TextAction::MoveCursor(-1),
                    ]);
                } else {
                    return Some(vec![TextAction::Commit("｛".to_string())]);
                }
            }
            '}' => {
                return Some(vec![TextAction::Commit("｝".to_string())]);
            }
            '"' => {
                if self.auto_complete_pairs {
                    return Some(vec![
                        TextAction::Commit("“”".to_string()),
                        TextAction::MoveCursor(-1),
                    ]);
                } else {
                    let quote_str = if self.double_quote_open {
                        self.double_quote_open = false;
                        "”"
                    } else {
                        self.double_quote_open = true;
                        "“"
                    };
                    return Some(vec![TextAction::Commit(quote_str.to_string())]);
                }
            }
            // 单引号（在独立标点输入模式下）
            '\'' => {
                if self.auto_complete_pairs {
                    return Some(vec![
                        TextAction::Commit("‘’".to_string()),
                        TextAction::MoveCursor(-1),
                    ]);
                } else {
                    let quote_str = if self.single_quote_open {
                        self.single_quote_open = false;
                        "’"
                    } else {
                        self.single_quote_open = true;
                        "‘"
                    };
                    return Some(vec![TextAction::Commit(quote_str.to_string())]);
                }
            }
            _ => {}
        }

        // 4. 常规中文标点全角映射
        match ch {
            ',' => Some(vec![TextAction::Commit("，".to_string())]),
            '.' => Some(vec![TextAction::Commit("。".to_string())]),
            '/' => {
                let s = if self.slash_to_dunhao { "、" } else { "/" };
                Some(vec![TextAction::Commit(s.to_string())])
            }
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

        // 数字后斜杠保护 2026/10/10
        pp.record_commit("2026");
        assert_eq!(
            pp.process_punctuation('/'),
            Some(vec![TextAction::Commit("/".to_string())])
        );

        // 网址模式标点直通
        assert_eq!(
            pp.process_punctuation_with_context('.', "www.baidu"),
            Some(vec![TextAction::Commit(".".to_string())])
        );
        assert_eq!(
            pp.process_punctuation_with_context('/', "https://github"),
            Some(vec![TextAction::Commit("/".to_string())])
        );
        assert_eq!(
            pp.process_punctuation_with_context('.', "user@example"),
            Some(vec![TextAction::Commit(".".to_string())])
        );
    }

    #[test]
    fn test_ctrl_toggle_and_punctuation_options() {
        let mut sm = ShiftToggleStateMachine::with_toggle_key(ToggleKeyMode::Ctrl);
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
            None
        );
        assert_eq!(
            sm.on_key_event(
                ShiftToggleStateMachine::KEY_LEFTCTRL,
                true,
                InputMode::Chinese
            ),
            None
        );
        assert_eq!(
            sm.on_key_event(
                ShiftToggleStateMachine::KEY_LEFTCTRL,
                false,
                InputMode::Chinese
            ),
            Some(InputMode::English)
        );

        let mut pp = PunctuationProcessor::with_options(false, false, false);
        assert_eq!(
            pp.process_punctuation('/'),
            Some(vec![TextAction::Commit("/".to_string())])
        );
        pp.record_commit("12");
        assert_eq!(
            pp.process_punctuation(':'),
            Some(vec![TextAction::Commit("：".to_string())])
        );
        assert_eq!(
            pp.process_punctuation('('),
            Some(vec![TextAction::Commit("（".to_string())])
        );

        // 验证非自动成对补全模式下，双引号交替输出 “ 与 ”
        assert_eq!(
            pp.process_punctuation('"'),
            Some(vec![TextAction::Commit("“".to_string())])
        );
        assert_eq!(
            pp.process_punctuation('"'),
            Some(vec![TextAction::Commit("”".to_string())])
        );
        assert_eq!(
            pp.process_punctuation('"'),
            Some(vec![TextAction::Commit("“".to_string())])
        );

        // 验证非自动成对补全模式下，单引号交替输出 ‘ 与 ’
        assert_eq!(
            pp.process_punctuation('\''),
            Some(vec![TextAction::Commit("‘".to_string())])
        );
        assert_eq!(
            pp.process_punctuation('\''),
            Some(vec![TextAction::Commit("’".to_string())])
        );
    }
}
