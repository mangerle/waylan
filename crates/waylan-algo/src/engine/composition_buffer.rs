//! 增量击键编辑缓冲区与预编辑切分子模型
//!
//! # 设计原理
//! - **实现初衷**：将单次输入击键缓冲、光标管理、Prism 隔音符切分缓存、分段自造词累积器以及跨句历史上下文
//!   封装为高内聚子模型。
//! - **核心优势**：解耦算法引擎核心流控与文本编辑、光标移动及分段选词状态，职责单一清晰。
//! - **代价与局限**：引擎操作光标与击键字符时需经由 `composition` 子模型代理。

use crate::composition::Composition;

/// 增量击键编辑缓冲区与预编辑切分子模型
#[derive(Debug, Default)]
pub struct CompositionBuffer {
    /// 基础敲击文本与光标状态机
    pub(crate) state: Composition,
    /// Prism 切分后的格式化预编辑文本缓存（避免渲染与光标定位重复执行 DAG 搜索）
    pub(crate) cached_preedit: String,
    /// 单次连续拼音输入过程中的分段选词累积器 (词条文本, 拼音片段)，用于整串消费完毕时自动合成用户自造新词
    pub(crate) partial_commit_segments: Vec<(String, String)>,
    /// 上一次选词上屏的历史上下文词条（用于跨句 Bigram 语言模型联想预测）
    pub(crate) last_commit_context: Option<String>,
}

impl CompositionBuffer {
    /// 创建新的击键编辑缓冲区
    pub fn new() -> Self {
        Self::default()
    }

    /// 键入一个字符，若在非末尾插入则清空分段自造词累积器
    pub fn push_char(&mut self, ch: char) {
        if self.state.cursor() < self.state.len() {
            self.partial_commit_segments.clear();
        }
        self.state.push_char(ch);
        self.cached_preedit.clear();
    }

    /// 退格删除字符，并清空分段自造词累积器
    pub fn pop_char(&mut self) {
        self.state.pop_char();
        self.partial_commit_segments.clear();
        self.cached_preedit.clear();
    }

    /// 向后删除光标右侧字符，并清空分段自造词累积器
    pub fn delete_char_at_cursor(&mut self) {
        self.state.delete_char_at_cursor();
        self.partial_commit_segments.clear();
        self.cached_preedit.clear();
    }

    /// 光标左移
    pub fn move_cursor_left(&mut self) -> bool {
        let changed = self.state.move_cursor_left();
        if changed {
            self.cached_preedit.clear();
        }
        changed
    }

    /// 光标右移
    pub fn move_cursor_right(&mut self) -> bool {
        let changed = self.state.move_cursor_right();
        if changed {
            self.cached_preedit.clear();
        }
        changed
    }

    /// 光标移动到末尾
    pub fn move_to_end(&mut self) -> bool {
        let changed = self.state.move_to_end();
        if changed {
            self.cached_preedit.clear();
        }
        changed
    }

    /// 消费前缀已选中的拼音片段长度
    pub fn consume_prefix(&mut self, len: usize) {
        self.state.consume_prefix(len);
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

    /// 获取当前输入文本
    pub fn as_str(&self) -> &str {
        self.state.as_str()
    }

    /// 当前光标位置
    pub fn cursor(&self) -> usize {
        self.state.cursor()
    }

    /// 输入文本字节长度
    pub fn len(&self) -> usize {
        self.state.len()
    }

    /// 输入文本是否为空
    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
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
        self.state.clear();
        self.cached_preedit.clear();
        self.partial_commit_segments.clear();
        self.last_commit_context = None;
    }
}
