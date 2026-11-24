//! 空闲态英文语境自动直通启发式识别器
//!
//! # 设计原理
//! - **实现初衷**：识别光标前紧贴的 URL、文件路径、蛇形/驼峰代码标识符等语境，
//!   在中文模式下自动临时放行英文与标点输入，减少频繁手动切换中英文的打断感。
//! - **核心优势**：纯切片零堆分配判定，独立于核心按键状态机，便于按需扩充规则与单元测试。
//! - **代价与局限**：仅基于光标前有限前缀文本做启发式判定，依赖前端提供周围文本上下文。

/// 判定光标前周围文本是否处于 URL、文件路径或代码标识符语境，决定空闲态是否自动临时直通英文
///
/// # 判定规则
/// 仅检查光标前紧贴的最后一个非空白 token（若光标紧跟空白或中文字符则返回 `false`，不干扰正常中文输入）：
/// 1. URL / 协议前缀（`http://`、`https://`、`ftp://`、`ssh://`、`file://`、`www.`）或邮箱（`@`）；
/// 2. 路径前缀或代码作用域符号（`/`、`./`、`../`、`~/`、`::`、`->`）；
/// 3. 蛇形变量（含 `_`）或成员访问/文件名（含 `.` 且相邻 ASCII 字母/数字）；
/// 4. 驼峰标识符（长度 >= 2 的纯 ASCII 字母数字串且同时包含大写与小写字母，如 `myVar`、`HashMap`）。
pub fn should_auto_passthrough_english(prefix: &str) -> bool {
    let Some(last_ch) = prefix.chars().next_back() else {
        return false;
    };
    if last_ch.is_whitespace() || !last_ch.is_ascii() {
        return false;
    }

    let token = prefix
        .rsplit(|c: char| c.is_whitespace() || !c.is_ascii())
        .next()
        .unwrap_or("");
    if token.is_empty() {
        return false;
    }

    if token.contains("://")
        || token.contains("::")
        || token.contains("->")
        || token.contains('@')
        || token.contains('_')
        || token.starts_with("www.")
        || token.starts_with('/')
        || token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
    {
        return true;
    }

    if let Some(dot_pos) = token.rfind('.') {
        let before_ascii = dot_pos > 0 && token.as_bytes()[dot_pos - 1].is_ascii_alphanumeric();
        let after_ascii = dot_pos + 1 == token.len()
            || token
                .as_bytes()
                .get(dot_pos + 1)
                .is_some_and(u8::is_ascii_alphanumeric);
        if before_ascii && after_ascii {
            return true;
        }
    }

    let is_ascii_ident = token.len() >= 2 && token.bytes().all(|b| b.is_ascii_alphanumeric());
    if is_ascii_ident {
        let has_upper = token.bytes().any(|b| b.is_ascii_uppercase());
        let has_lower = token.bytes().any(|b| b.is_ascii_lowercase());
        if has_upper && has_lower {
            return true;
        }
    }

    false
}
