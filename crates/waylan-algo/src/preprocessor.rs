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
    /// 是否开启全角标点映射（true 为全角中文标点，false 为半角英文标点）
    pub full_width_punct: bool,
    /// 最近提交或输入的上下文文本缓存（用于无外部应用上下文时感知网址与邮箱模式）
    pub recent_context: String,
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
            full_width_punct: true,
            recent_context: String::with_capacity(128),
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
            full_width_punct: true,
            recent_context: String::with_capacity(128),
        }
    }

    /// 从新配置实例同步标点特性布尔开关，同时完整保留当前引号开闭状态与最近输入上下文
    pub fn apply_options_from(&mut self, other: &Self) {
        self.slash_to_dunhao = other.slash_to_dunhao;
        self.protect_digits_punctuation = other.protect_digits_punctuation;
        self.auto_complete_pairs = other.auto_complete_pairs;
        self.full_width_punct = other.full_width_punct;
    }

    /// 重置成对引号的开闭记忆状态
    pub fn reset_quote_states(&mut self) {
        self.double_quote_open = false;
        self.single_quote_open = false;
    }

    /// 重置标点上下文与成对引号开闭状态（用于窗口焦点切换或会话失焦）
    pub fn reset(&mut self) {
        self.last_char_was_digit = false;
        self.double_quote_open = false;
        self.single_quote_open = false;
        self.recent_context.clear();
    }

    /// 记录外部提交动作以更新上下文状态（维护数字状态、最近输入上下文与成对引号开闭状态）
    pub fn record_commit(&mut self, text: &str) {
        if let Some(last_ch) = text.chars().last() {
            self.last_char_was_digit = last_ch.is_ascii_digit();
        }
        self.recent_context.push_str(text);
        if self.recent_context.len() > 128 {
            let keep_start = self.recent_context.len().saturating_sub(64);
            let safe_start = self
                .recent_context
                .char_indices()
                .map(|(i, _)| i)
                .find(|&i| i >= keep_start)
                .unwrap_or(0);
            self.recent_context = self.recent_context[safe_start..].to_string();
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

    /// 检查输入缓冲区或上下文前缀末尾是否属于网址模式或邮箱模式
    pub fn is_url_or_email_mode(buffer: &str) -> bool {
        let trimmed = buffer.trim_end();
        if trimmed.is_empty() {
            return false;
        }
        // 若末尾字符不是 ASCII 或为空白，光标前紧邻的不是网络词法单元，直接返回 false
        let last_char = match trimmed.chars().next_back() {
            Some(c) => c,
            None => return false,
        };
        if !last_char.is_ascii() || last_char.is_whitespace() {
            return false;
        }

        // 提取末尾紧邻的连续非空白 ASCII Token
        let token_start = trimmed
            .rfind(|c: char| c.is_whitespace() || !c.is_ascii())
            .map(|i| {
                let ch_len = trimmed[i..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1);
                i + ch_len
            })
            .unwrap_or(0);
        let last_token = &trimmed[token_start..];
        if last_token.is_empty() {
            return false;
        }
        let lower = last_token.to_lowercase();
        lower.contains("://")
            || lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("ftp://")
            || lower.starts_with("file://")
            || lower.starts_with("www.")
            || lower.starts_with("git@")
            || lower.contains('@')
            || lower.ends_with("http")
            || lower.ends_with("https")
            || lower.ends_with("ftp")
            || lower.ends_with("http:")
            || lower.ends_with("https:")
            || lower.ends_with("ftp:")
            || lower.ends_with("http:/")
            || lower.ends_with("https:/")
            || lower.ends_with("ftp:/")
            || lower.ends_with("www")
            || lower.ends_with(".com")
            || lower.ends_with(".cn")
            || lower.ends_with(".org")
            || lower.ends_with(".net")
            || lower.ends_with(".io")
            || lower.ends_with(".edu")
            || lower.ends_with(".gov")
    }

    /// 尝试处理标点符号输入（自动从内置 recent_context 获取上下文）
    pub fn process_punctuation(&mut self, ch: char) -> Option<Vec<TextAction>> {
        let ctx = self.recent_context.clone();
        self.process_punctuation_with_context(ch, &ctx)
    }

    /// 结合当前输入缓冲区或应用上下文处理标点符号输入
    pub fn process_punctuation_with_context(
        &mut self,
        ch: char,
        buffer: &str,
    ) -> Option<Vec<TextAction>> {
        // 0. 半角标点模式：直接提交原始半角英文字符，不触发中文全角映射与成对符号转换
        if !self.full_width_punct {
            self.last_char_was_digit = false;
            return Some(vec![TextAction::Commit(ch.to_string())]);
        }

        // 1. 网址与邮箱模式感知：当前缓冲区末尾为 URL 或 Email 时，常见网络标点强制保持半角 ASCII
        if Self::is_url_or_email_mode(buffer) && ".:/_?#&=@~-+".contains(ch) {
            self.last_char_was_digit = false;
            return Some(vec![TextAction::Commit(ch.to_string())]);
        }

        // 2. 数字间/数字后符号保护（例如 12:00, 3.14, 2026/10/10，保持英文半角符号）
        if self.protect_digits_punctuation
            && self.last_char_was_digit
            && (ch == ':' || ch == '.' || ch == '/')
        {
            self.last_char_was_digit = false;
            return Some(vec![TextAction::Commit(ch.to_string())]);
        }

        self.last_char_was_digit = false;

        // 3. 成对符号处理
        if let Some(actions) = self.handle_paired_punctuation(ch) {
            return Some(actions);
        }

        // 4. 常规中文标点全角映射
        self.handle_regular_punctuation(ch)
    }

    /// 处理括号与引号等成对符号
    fn handle_paired_punctuation(&mut self, ch: char) -> Option<Vec<TextAction>> {
        match ch {
            '(' => Some(if self.auto_complete_pairs {
                vec![
                    TextAction::Commit("（）".to_string()),
                    TextAction::MoveCursor(-1),
                ]
            } else {
                vec![TextAction::Commit("（".to_string())]
            }),
            ')' => Some(vec![TextAction::Commit("）".to_string())]),
            '<' => Some(if self.auto_complete_pairs {
                vec![
                    TextAction::Commit("《》".to_string()),
                    TextAction::MoveCursor(-1),
                ]
            } else {
                vec![TextAction::Commit("《".to_string())]
            }),
            '>' => Some(vec![TextAction::Commit("》".to_string())]),
            '[' => Some(if self.auto_complete_pairs {
                vec![
                    TextAction::Commit("【】".to_string()),
                    TextAction::MoveCursor(-1),
                ]
            } else {
                vec![TextAction::Commit("【".to_string())]
            }),
            ']' => Some(vec![TextAction::Commit("】".to_string())]),
            '{' => Some(if self.auto_complete_pairs {
                vec![
                    TextAction::Commit("｛｝".to_string()),
                    TextAction::MoveCursor(-1),
                ]
            } else {
                vec![TextAction::Commit("｛".to_string())]
            }),
            '}' => Some(vec![TextAction::Commit("｝".to_string())]),
            '"' => {
                if self.auto_complete_pairs {
                    Some(vec![
                        TextAction::Commit("“”".to_string()),
                        TextAction::MoveCursor(-1),
                    ])
                } else {
                    let quote_str = if self.double_quote_open {
                        self.double_quote_open = false;
                        "”"
                    } else {
                        self.double_quote_open = true;
                        "“"
                    };
                    Some(vec![TextAction::Commit(quote_str.to_string())])
                }
            }
            '\'' => {
                if self.auto_complete_pairs {
                    Some(vec![
                        TextAction::Commit("‘’".to_string()),
                        TextAction::MoveCursor(-1),
                    ])
                } else {
                    let quote_str = if self.single_quote_open {
                        self.single_quote_open = false;
                        "’"
                    } else {
                        self.single_quote_open = true;
                        "‘"
                    };
                    Some(vec![TextAction::Commit(quote_str.to_string())])
                }
            }
            _ => None,
        }
    }

    /// 处理常规单字符全角中文标点映射
    fn handle_regular_punctuation(&self, ch: char) -> Option<Vec<TextAction>> {
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
