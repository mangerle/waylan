//! 增量击键编辑缓冲区与预编辑组合状态机
//!
//! # 设计原理
//! - **实现初衷**：将按键字符输入缓冲、光标管理、Prism 隔音符切分缓存、分段自造词累积器以及跨句历史上下文
//!   直接内聚为单一模型，消除原本 `Composition` 与 `CompositionBuffer` 之间的冗余代理包裹层。
//! - **核心优势**：单结构体持有状态，消除 12 个二级转发方法，光标越界防御性处理与预编辑缓存清空紧密内聚。
//! - **代价与局限**：模型集成了文本编辑与分段上屏累积两个关注点。

use crate::error::AlgoError;

/// 增量击键编辑缓冲区与预编辑切分子模型
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompositionBuffer {
    /// 用户当前键入的原始英文字符缓冲区
    pub(crate) raw_buffer: String,
    /// 插入光标在缓冲区中的字节偏移位置
    pub(crate) cursor_position: usize,
    /// Prism 切分后的格式化预编辑文本缓存（避免渲染与光标定位重复执行 DAG 搜索）
    pub(crate) cached_preedit: String,
    /// 单次连续拼音输入过程中的分段选词累积器 (词条文本, 拼音片段)，用于整串消费完毕时自动合成用户自造新词
    pub(crate) partial_commit_segments: Vec<(String, String)>,
    /// 上一次选词上屏的历史上下文词条（用于跨句 Bigram 语言模型联想预测）
    pub(crate) last_commit_context: Option<String>,
}

impl CompositionBuffer {
    /// 创建新的空击键编辑缓冲区
    pub fn new() -> Self {
        Self::default()
    }

    /// 基于给定初始输入内容构造编辑缓冲区
    pub fn with_buffer(raw: impl Into<String>) -> Self {
        let raw_buffer = raw.into();
        let cursor_position = raw_buffer.len();
        Self {
            raw_buffer,
            cursor_position,
            cached_preedit: String::new(),
            partial_commit_segments: Vec::new(),
            last_commit_context: None,
        }
    }

    /// 键入一个字符，若在非末尾插入则清空分段自造词累积器
    pub fn push_char(&mut self, ch: char) {
        if self.cursor_position < self.raw_buffer.len() {
            self.partial_commit_segments.clear();
            self.raw_buffer.insert(self.cursor_position, ch);
        } else {
            self.raw_buffer.push(ch);
        }
        self.cursor_position = self.cursor_position.saturating_add(ch.len_utf8());
        self.cached_preedit.clear();
    }

    /// 退格删除光标前的一个字符（Backspace 行为），并清空分段自造词累积器与预编辑缓存
    pub fn pop_char(&mut self) -> Option<char> {
        if self.cursor_position == 0 || self.raw_buffer.is_empty() {
            return None;
        }

        let mut prev_char_idx = self.cursor_position - 1;
        while prev_char_idx > 0 && !self.raw_buffer.is_char_boundary(prev_char_idx) {
            prev_char_idx = prev_char_idx.saturating_sub(1);
        }

        let removed = self.raw_buffer.remove(prev_char_idx);
        self.cursor_position = prev_char_idx;
        self.partial_commit_segments.clear();
        self.cached_preedit.clear();
        Some(removed)
    }

    /// 向后删除光标右侧的一个字符（Delete 行为），并清空分段自造词累积器与预编辑缓存
    pub fn delete_char_at_cursor(&mut self) -> Option<char> {
        if self.cursor_position >= self.raw_buffer.len() {
            return None;
        }

        let removed = self.raw_buffer.remove(self.cursor_position);
        self.partial_commit_segments.clear();
        self.cached_preedit.clear();
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
        self.cached_preedit.clear();
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
        self.cached_preedit.clear();
        true
    }

    /// 将插入光标直接移动至缓冲区末尾，返回光标是否发生有效位移
    pub fn move_to_end(&mut self) -> bool {
        if self.cursor_position < self.raw_buffer.len() {
            self.cursor_position = self.raw_buffer.len();
            self.cached_preedit.clear();
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
        self.cached_preedit.clear();
        Ok(())
    }

    /// 分段确认上屏：消费前缀 consumed_chars_len 长度的拼音，保留剩余未选拼音
    pub fn consume_prefix(&mut self, consumed_chars_len: usize) {
        if consumed_chars_len >= self.raw_buffer.len() {
            self.raw_buffer.clear();
            self.cursor_position = 0;
        } else {
            let safe_cut_len = if self.raw_buffer.is_char_boundary(consumed_chars_len) {
                consumed_chars_len
            } else {
                (consumed_chars_len..=self.raw_buffer.len())
                    .find(|&idx| self.raw_buffer.is_char_boundary(idx))
                    .unwrap_or(self.raw_buffer.len())
            };

            if safe_cut_len >= self.raw_buffer.len() {
                self.raw_buffer.clear();
                self.cursor_position = 0;
            } else {
                let remaining = self.raw_buffer[safe_cut_len..].to_string();
                self.raw_buffer = remaining;
                self.cursor_position = self.raw_buffer.len();
            }
        }
        self.cached_preedit.clear();
    }

    /// 记录分段选词片段 (词条文本, 对应拼音)
    pub fn record_partial_segment(&mut self, text: String, pinyin: String) {
        self.partial_commit_segments.push((text, pinyin));
    }

    /// 获取分段选词累积器切片
    pub fn partial_segments(&self) -> &[(String, String)] {
        &self.partial_commit_segments
    }

    /// 清空分段选词累积器
    pub fn clear_partial_segments(&mut self) {
        self.partial_commit_segments.clear();
    }

    /// 获取当前原始输入字符串切片
    pub fn as_str(&self) -> &str {
        &self.raw_buffer
    }

    /// 获取当前光标位置
    pub fn cursor(&self) -> usize {
        self.cursor_position
    }

    /// 获取当前输入字符长度
    pub fn len(&self) -> usize {
        self.raw_buffer.len()
    }

    /// 输入缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.raw_buffer.is_empty()
    }

    /// 获取预编辑格式化缓存
    pub fn cached_preedit(&self) -> &str {
        &self.cached_preedit
    }

    /// 设置预编辑格式化缓存
    pub fn set_cached_preedit(&mut self, s: String) {
        self.cached_preedit = s;
    }

    /// 清空预编辑格式化缓存
    pub fn clear_cached_preedit(&mut self) {
        self.cached_preedit.clear();
    }

    /// 获取上一轮上屏的上下文词
    pub fn last_commit_context(&self) -> Option<&str> {
        self.last_commit_context.as_deref()
    }

    /// 设置上一轮上屏的上下文词
    pub fn set_last_commit_context(&mut self, ctx: Option<String>) {
        self.last_commit_context = ctx;
    }

    /// 清空上屏历史上下文缓存
    pub fn clear_commit_context(&mut self) {
        self.last_commit_context = None;
    }

    /// 清空并重置输入缓冲区与全部累积状态
    pub fn reset(&mut self) {
        self.raw_buffer.clear();
        self.cursor_position = 0;
        self.cached_preedit.clear();
        self.partial_commit_segments.clear();
        self.last_commit_context = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_pop() {
        let mut comp = CompositionBuffer::new();
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
        let mut comp = CompositionBuffer::new();
        comp.push_char('a');
        comp.push_char('c');
        assert_eq!(comp.as_str(), "ac");

        assert!(comp.move_cursor_left());
        comp.push_char('b');
        assert_eq!(comp.as_str(), "abc");
        assert_eq!(comp.cursor(), 2);

        assert!(comp.move_cursor_right());
        assert_eq!(comp.cursor(), 3);
        assert!(!comp.move_cursor_right());
    }

    #[test]
    fn test_delete_char_at_cursor() {
        let mut comp = CompositionBuffer::with_buffer("abc");
        assert!(comp.set_cursor(1).is_ok());
        let deleted = comp.delete_char_at_cursor();
        assert_eq!(deleted, Some('b'));
        assert_eq!(comp.as_str(), "ac");
        assert_eq!(comp.cursor(), 1);
    }

    #[test]
    fn test_consume_prefix() {
        let mut comp = CompositionBuffer::new();
        for ch in "nihao".chars() {
            comp.push_char(ch);
        }
        comp.consume_prefix(2);
        assert_eq!(comp.as_str(), "hao");
        assert_eq!(comp.cursor(), 3);
    }

    #[test]
    fn test_consume_prefix_non_char_boundary_no_panic() {
        let mut comp = CompositionBuffer::new();
        for ch in "测nihao".chars() {
            comp.push_char(ch);
        }
        comp.consume_prefix(1);
        assert_eq!(comp.as_str(), "nihao");
        assert_eq!(comp.cursor(), 5);
    }

    #[test]
    fn test_with_buffer_and_move_to_end() {
        let mut comp = CompositionBuffer::with_buffer("nihao");
        assert_eq!(comp.as_str(), "nihao");
        assert_eq!(comp.cursor(), 5);

        assert!(comp.move_cursor_left());
        assert!(comp.move_cursor_left());
        assert_eq!(comp.cursor(), 3);
        assert!(comp.move_to_end());
        assert_eq!(comp.cursor(), 5);
        assert!(!comp.move_to_end());
    }

    #[test]
    fn test_partial_segments_and_reset() {
        let mut comp = CompositionBuffer::new();
        comp.push_char('n');
        comp.record_partial_segment("你".to_string(), "ni".to_string());
        comp.set_cached_preedit("n'i".to_string());
        comp.set_last_commit_context(Some("你好".to_string()));

        assert_eq!(comp.partial_segments().len(), 1);
        assert_eq!(comp.cached_preedit(), "n'i");
        assert_eq!(comp.last_commit_context(), Some("你好"));

        comp.reset();
        assert!(comp.is_empty());
        assert_eq!(comp.cursor(), 0);
        assert!(comp.cached_preedit().is_empty());
        assert!(comp.partial_segments().is_empty());
        assert_eq!(comp.last_commit_context(), None);
    }
}
