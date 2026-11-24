//! 中文智能标点符号处理器
//!
//! # 设计原理
//! - **实现初衷**：提供舒适体贴的智能标点交互细节（斜杠自动转顿号、成对括号自动闭合左移光标、数字间冒号小数点保护）。
//! - **核心优势**：在拼音引擎外层形成纯函数式拦截，打数字（如 12:30 或 3.14）时无需切英文模式。
//! - **代价与局限**：需由外部在选词上屏时调用 `record_commit` 维护前驱字符上下文。

use waylan_core::TextAction;
use waylan_core::config::PinyinConfig;

/// 智能标点符号处理选项（参数对象）
///
/// # 设计原理
/// - **实现初衷**：将散落的多个标点特性布尔开关收敛为内聚的参数对象，消除长参数构造方法与布尔盲视。
/// - **核心优势**：支持整体原子化配置更新与快照比对，并可无缝对接 `PinyinConfig`。
/// - **代价与局限**：调用方若仅需调整单项参数，需通过结构体更新语法或对应专属方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PunctuationOptions {
    /// 是否将斜杠 / 转换为中文顿号 、
    pub slash_to_dunhao: bool,
    /// 是否将反斜杠 \ 转换为中文顿号 、
    pub backslash_to_dunhao: bool,
    /// 是否开启数字后紧跟冒号/小数点/斜杠自动保持英文半角保护
    pub protect_digits_punctuation: bool,
    /// 是否开启成对括号与引号自动补全并回退光标
    pub auto_complete_pairs: bool,
    /// 是否开启全角标点映射（true 为全角中文标点，false 为半角英文标点）
    pub full_width_punct: bool,
}

impl Default for PunctuationOptions {
    fn default() -> Self {
        Self {
            slash_to_dunhao: true,
            backslash_to_dunhao: true,
            protect_digits_punctuation: true,
            auto_complete_pairs: true,
            full_width_punct: true,
        }
    }
}

impl From<&PinyinConfig> for PunctuationOptions {
    fn from(cfg: &PinyinConfig) -> Self {
        Self {
            slash_to_dunhao: cfg.slash_to_dunhao,
            backslash_to_dunhao: cfg.backslash_to_dunhao,
            protect_digits_punctuation: cfg.protect_digits_punctuation,
            auto_complete_pairs: cfg.auto_complete_pairs,
            full_width_punct: cfg.full_width_punct,
        }
    }
}

/// 智能标点符号处理器
#[derive(Debug, Clone)]
pub struct PunctuationProcessor {
    /// 标点符号处理器配置选项
    pub options: PunctuationOptions,
    /// 记录上一次提交的字符是否是数字
    last_char_was_digit: bool,
    /// 双引号开闭状态位（false 为待输入左双引号，true 为已输入左双引号待输入右双引号）
    pub double_quote_open: bool,
    /// 单引号开闭状态位（false 为待输入左单引号，true 为已输入左单引号待输入右单引号）
    pub single_quote_open: bool,
    /// 最近提交或输入的上下文文本缓存（用于无外部应用上下文时感知网址与邮箱模式）
    pub recent_context: String,
}

impl Default for PunctuationProcessor {
    fn default() -> Self {
        Self {
            options: PunctuationOptions::default(),
            last_char_was_digit: false,
            double_quote_open: false,
            single_quote_open: false,
            recent_context: String::with_capacity(128),
        }
    }
}

impl PunctuationProcessor {
    /// 创建处于初始空状态的智能标点处理器（与 `Default` 等价，保留以匹配构造器命名规范）
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用指定的标点特性配置选项构造处理器
    pub fn with_options(options: PunctuationOptions) -> Self {
        Self {
            options,
            last_char_was_digit: false,
            double_quote_open: false,
            single_quote_open: false,
            recent_context: String::with_capacity(128),
        }
    }

    /// 获取当前标点特性配置选项的不可变引用
    #[inline]
    pub fn options(&self) -> &PunctuationOptions {
        &self.options
    }

    /// 获取当前标点特性配置选项的可变引用
    #[inline]
    pub fn options_mut(&mut self) -> &mut PunctuationOptions {
        &mut self.options
    }

    /// 整体更新标点特性配置选项（完整保留当前引号开闭状态与最近输入上下文）
    pub fn set_options(&mut self, options: PunctuationOptions) {
        self.options = options;
    }

    /// 从另一处理器实例同步标点特性配置选项（完整保留当前引号开闭状态与最近输入上下文）
    pub fn apply_options_from(&mut self, other: &Self) {
        self.options = other.options;
    }

    /// 获取当前是否开启全角标点映射
    #[inline]
    pub fn is_full_width_punct(&self) -> bool {
        self.options.full_width_punct
    }

    /// 设置全角标点映射开关
    #[inline]
    pub fn set_full_width_punct(&mut self, full_width_punct: bool) {
        self.options.full_width_punct = full_width_punct;
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
            self.recent_context.drain(..safe_start);
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

        // 提取末尾紧邻的连续非空白 ASCII Token，优先在栈缓冲内完成小写规整避免堆分配
        let last_token = trailing_ascii_token(trimmed);
        if last_token.is_empty() {
            return false;
        }
        if last_token.len() <= 128 {
            let mut buf = [0u8; 128];
            let len = last_token.len();
            for (dst, &src) in buf[..len].iter_mut().zip(last_token.as_bytes()) {
                *dst = src.to_ascii_lowercase();
            }
            if let Ok(lower) = std::str::from_utf8(&buf[..len]) {
                return contains_url_scheme(lower) || contains_domain_suffix(lower);
            }
        }
        let lower = last_token.to_ascii_lowercase();
        contains_url_scheme(&lower) || contains_domain_suffix(&lower)
    }

    /// 尝试处理标点符号输入（自动从内置 recent_context 获取上下文，零堆分配）
    pub fn process_punctuation(&mut self, ch: char) -> Option<Vec<TextAction>> {
        let ctx = std::mem::take(&mut self.recent_context);
        let res = self.process_punctuation_with_context(ch, &ctx);
        self.recent_context = ctx;
        res
    }

    /// 结合当前输入缓冲区或应用上下文处理标点符号输入
    pub fn process_punctuation_with_context(
        &mut self,
        ch: char,
        buffer: &str,
    ) -> Option<Vec<TextAction>> {
        // 0. 半角标点模式：直接提交原始半角英文字符，不触发中文全角映射与成对符号转换
        if !self.options.full_width_punct {
            self.last_char_was_digit = false;
            return Some(vec![TextAction::Commit(ch.to_string())]);
        }

        // 1. 网址与邮箱模式感知：当前缓冲区末尾为 URL 或 Email 时，常见网络标点强制保持半角 ASCII
        if Self::is_url_or_email_mode(buffer) && ".:/_?#&=@~-+".contains(ch) {
            self.last_char_was_digit = false;
            return Some(vec![TextAction::Commit(ch.to_string())]);
        }

        // 2. 数字间/数字后符号保护（例如 12:00, 3.14, 2026/10/10，保持英文半角符号）
        if self.options.protect_digits_punctuation
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
            '(' => Some(self.paired_symbol_actions("（）", "（")),
            ')' => Some(vec![TextAction::Commit("）".to_string())]),
            '<' => Some(self.paired_symbol_actions("《》", "《")),
            '>' => Some(vec![TextAction::Commit("》".to_string())]),
            '[' => Some(self.paired_symbol_actions("【】", "【")),
            ']' => Some(vec![TextAction::Commit("】".to_string())]),
            '{' => Some(self.paired_symbol_actions("｛｝", "｛")),
            '}' => Some(vec![TextAction::Commit("｝".to_string())]),
            '"' => {
                if self.options.auto_complete_pairs {
                    Some(self.paired_symbol_actions("“”", "“"))
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
                if self.options.auto_complete_pairs {
                    Some(self.paired_symbol_actions("‘’", "‘"))
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

    /// 依据自动补全开关生成成对符号的提交动作（自动补全时提交完整配对并将光标回移至中间位）
    fn paired_symbol_actions(&self, pair: &str, single: &str) -> Vec<TextAction> {
        if self.options.auto_complete_pairs {
            vec![
                TextAction::Commit(pair.to_string()),
                TextAction::MoveCursor(-1),
            ]
        } else {
            vec![TextAction::Commit(single.to_string())]
        }
    }
    /// 处理常规单字符全角中文标点映射
    fn handle_regular_punctuation(&self, ch: char) -> Option<Vec<TextAction>> {
        match ch {
            ',' => Some(vec![TextAction::Commit("，".to_string())]),
            '.' => Some(vec![TextAction::Commit("。".to_string())]),
            '/' => {
                let s = if self.options.slash_to_dunhao {
                    "、"
                } else {
                    "/"
                };
                Some(vec![TextAction::Commit(s.to_string())])
            }
            '\\' => {
                let s = if self.options.backslash_to_dunhao {
                    "、"
                } else {
                    "\\"
                };
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
            '|' => Some(vec![TextAction::Commit("|".to_string())]),
            _ => None,
        }
    }
}

/// 提取文本末尾紧邻的连续非空白 ASCII 词法单元
///
/// # 设计原理
/// - 以"空白或非 ASCII 字符"作为 Token 分隔边界，确保只对光标前的完整网络词法单元做判定，
///   避免把中文句子里的单个英文字母误判为网址模式。
fn trailing_ascii_token(trimmed: &str) -> &str {
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
    &trimmed[token_start..]
}

/// 判断词法单元是否具备 URL 协议、常见协议前缀或邮箱特征
///
/// # 设计原理
/// - 覆盖"显式协议"（含 `://`）、"裸域名"（`www.` 开头）与"账号@主机"（邮箱）三类主流网络词法。
fn contains_url_scheme(lower: &str) -> bool {
    lower.contains("://")
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("file://")
        || lower.starts_with("www.")
        || lower.starts_with("git@")
        || lower.contains('@')
}

/// 判断词法单元是否以常见协议名、协议残尾或顶级域名结尾
///
/// # 设计原理
/// - 覆盖"用户已输入部分协议但尚未补全分隔符"（如 `http`、`https:/`）与常见顶级域名后缀，
///   使半成品网址同样保持半角标点。
fn contains_domain_suffix(lower: &str) -> bool {
    lower.ends_with("http")
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
