use crate::error::AlgoError;

/// 增量敲击编辑与组合状态机 (Composition State Machine)
///
/// # 设计原理
/// - **实现初衷**：负责跟踪用户动态敲击的字母字符流、光标插入点以及分段确认操作。
/// - **核心优势**：支持分段上屏（Partial Commit，如一长串拼音中先选前两个字，剩余拼音无缝保留继续选词），
///   并在内部确保光标操作的 UTF-8 边界安全。
/// - **代价与局限**：维护光标移动与字符增删状态，需防御性防范索引越界。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Composition {
    /// 用户当前键入的原始英文字符缓冲区
    raw_buffer: String,
    /// 插入光标在缓冲区中的字节偏移位置
    cursor_position: usize,
}

impl Composition {
    /// 创建新的空编辑状态机
    pub fn new() -> Self {
        Self {
            raw_buffer: String::new(),
            cursor_position: 0,
        }
    }

    /// 基于给定初始输入内容构造编辑状态机
    pub fn with_buffer(raw: impl Into<String>) -> Self {
        let raw_buffer = raw.into();
        let cursor_position = raw_buffer.len();
        Self {
            raw_buffer,
            cursor_position,
        }
    }

    /// 在当前光标位置插入字符
    pub fn push_char(&mut self, ch: char) {
        if self.cursor_position == self.raw_buffer.len() {
            self.raw_buffer.push(ch);
        } else {
            self.raw_buffer.insert(self.cursor_position, ch);
        }
        self.cursor_position = self.cursor_position.saturating_add(ch.len_utf8());
    }

    /// 删除光标前的一个字符（Backspace 行为）
    pub fn pop_char(&mut self) -> Option<char> {
        if self.cursor_position == 0 || self.raw_buffer.is_empty() {
            return None;
        }

        // 寻找光标前一个字符的起始索引
        let mut prev_char_idx = self.cursor_position - 1;
        while prev_char_idx > 0 && !self.raw_buffer.is_char_boundary(prev_char_idx) {
            prev_char_idx = prev_char_idx.saturating_sub(1);
        }

        let removed = self.raw_buffer.remove(prev_char_idx);
        self.cursor_position = prev_char_idx;
        Some(removed)
    }

    /// 删除光标后（右侧）的一个字符（Delete 键行为）
    pub fn delete_char_at_cursor(&mut self) -> Option<char> {
        if self.cursor_position >= self.raw_buffer.len() {
            return None;
        }

        let removed = self.raw_buffer.remove(self.cursor_position);
        Some(removed)
    }

    /// 光标向左移动一个字符
    pub fn move_cursor_left(&mut self) -> bool {
        if self.cursor_position == 0 {
            return false;
        }
        let mut new_pos = self.cursor_position - 1;
        while new_pos > 0 && !self.raw_buffer.is_char_boundary(new_pos) {
            new_pos = new_pos.saturating_sub(1);
        }
        self.cursor_position = new_pos;
        true
    }

    /// 光标向右移动一个字符
    pub fn move_cursor_right(&mut self) -> bool {
        if self.cursor_position >= self.raw_buffer.len() {
            return false;
        }
        let mut new_pos = self.cursor_position + 1;
        while new_pos < self.raw_buffer.len() && !self.raw_buffer.is_char_boundary(new_pos) {
            new_pos = new_pos.saturating_add(1);
        }
        self.cursor_position = new_pos;
        true
    }

    /// 将插入光标直接移动至缓冲区末尾，返回光标是否发生有效位移
    pub fn move_to_end(&mut self) -> bool {
        if self.cursor_position < self.raw_buffer.len() {
            self.cursor_position = self.raw_buffer.len();
            true
        } else {
            false
        }
    }

    /// 设置光标到指定位置
    ///
    /// # Errors
    /// 当目标位置超出有效字符串长度或不是合法字符边界时返回 [`AlgoError::InvalidCursorPosition`]。
    pub fn set_cursor(&mut self, pos: usize) -> Result<(), AlgoError> {
        if pos > self.raw_buffer.len() || !self.raw_buffer.is_char_boundary(pos) {
            return Err(AlgoError::InvalidCursorPosition {
                current_len: self.raw_buffer.len(),
                target_pos: pos,
            });
        }
        self.cursor_position = pos;
        Ok(())
    }

    /// 分段确认上屏：消费前缀 consumed_chars 长度的拼音，保留剩余未选拼音
    pub fn consume_prefix(&mut self, consumed_chars_len: usize) {
        if consumed_chars_len >= self.raw_buffer.len() {
            self.clear();
        } else {
            // 切除已上屏的前缀部分，防止非字符边界导致 panic
            let safe_cut_len = if self.raw_buffer.is_char_boundary(consumed_chars_len) {
                consumed_chars_len
            } else {
                // 若给定的字节偏移不是合法的 UTF-8 字符边界，寻找向后最近的字符边界；若无则清空
                (consumed_chars_len..=self.raw_buffer.len())
                    .find(|&idx| self.raw_buffer.is_char_boundary(idx))
                    .unwrap_or(self.raw_buffer.len())
            };

            if safe_cut_len >= self.raw_buffer.len() {
                self.clear();
            } else {
                let remaining = self.raw_buffer[safe_cut_len..].to_string();
                self.raw_buffer = remaining;
                self.cursor_position = self.raw_buffer.len();
            }
        }
    }

    /// 清空所有输入
    pub fn clear(&mut self) {
        self.raw_buffer.clear();
        self.cursor_position = 0;
    }

    /// 获取当前原始输入字符串切片
    pub fn as_str(&self) -> &str {
        &self.raw_buffer
    }

    /// 获取当前输入字符长度
    pub fn len(&self) -> usize {
        self.raw_buffer.len()
    }

    /// 输入缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.raw_buffer.is_empty()
    }

    /// 获取当前光标位置
    pub fn cursor(&self) -> usize {
        self.cursor_position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_pop() {
        let mut comp = Composition::new();
        comp.push_char('n');
        comp.push_char('i');
        assert_eq!(comp.as_str(), "ni");
        assert_eq!(comp.cursor(), 2);

        let popped = comp.pop_char();
        assert_eq!(popped, Some('i'));
        assert_eq!(comp.as_str(), "n");
        assert_eq!(comp.cursor(), 1);
    }

    #[test]
    fn test_cursor_movement_and_insert() {
        let mut comp = Composition::new();
        comp.push_char('a');
        comp.push_char('c');
        assert_eq!(comp.as_str(), "ac");

        comp.move_cursor_left();
        comp.push_char('b');
        assert_eq!(comp.as_str(), "abc");
    }

    #[test]
    fn test_consume_prefix() {
        let mut comp = Composition::new();
        for ch in "nihao".chars() {
            comp.push_char(ch);
        }
        // 假设“你”对应“ni”，消费 2 个字母
        comp.consume_prefix(2);
        assert_eq!(comp.as_str(), "hao");
        assert_eq!(comp.cursor(), 3);
    }

    #[test]
    fn test_consume_prefix_non_char_boundary_no_panic() {
        let mut comp = Composition::new();
        for ch in "测nihao".chars() {
            comp.push_char(ch);
        }
        // "测" 是 3 字节 UTF-8，传入偏移 1 或 2 是非字符边界，断言不会 panic 且安全对齐
        comp.consume_prefix(1);
        assert_eq!(comp.as_str(), "nihao");
        assert_eq!(comp.cursor(), 5);
    }

    #[test]
    fn test_with_buffer() {
        let comp = Composition::with_buffer("nihao");
        assert_eq!(comp.as_str(), "nihao");
        assert_eq!(comp.cursor(), 5);
    }

    #[test]
    fn test_move_to_end() {
        let mut comp = Composition::with_buffer("nihao");
        comp.move_cursor_left();
        comp.move_cursor_left();
        assert_eq!(comp.cursor(), 3);
        assert!(comp.move_to_end());
        assert_eq!(comp.cursor(), 5);
        assert!(!comp.move_to_end());
    }
}
