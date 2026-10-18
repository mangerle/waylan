pub mod adaptive;
pub mod candidates;
pub mod composition_buffer;
pub(crate) mod prefix_synthesis;
pub mod resources;
pub mod selection;
pub mod typo;
pub mod viewport;

#[cfg(test)]
mod tests;

pub use adaptive::UserAdaptiveState;
pub use composition_buffer::CompositionBuffer;
pub use resources::LexiconResources;
pub use viewport::CandidateViewport;
pub use waylan_core::PanguSpacing;

use std::borrow::Cow;
use std::path::Path;

use crate::custom_phrase::AuxiliaryDict;
use crate::error::AlgoError;
use crate::prism::PrismEngine;
use crate::table::LexiconTable;
use crate::types::{AlgoCandidate, AlgoConfig, ImportedDictEntry};

/// 微澜统一核心算法引擎门面 (AlgoEngine)
///
/// # 设计原理
/// - **实现初衷**：将模式分流、DAG 拼写图、词典倒排、Viterbi 解码、自适应学习与后处理流水线封装为高内聚门面，
///   向输入法上层提供极简的按键响应与选词接口。
/// - **核心优势**：完全纯 Rust 实现，零 C++ 依赖；全生命周期无锁、无异步竞态；100% 内存安全与可测试。
/// - **解构架构**：解构为高内聚的四大子系统：
///   1. [`LexiconResources`]：只读系统词典、实体保护、自定义短语及简繁字典资源；
///   2. [`CandidateViewport`]：分页高亮、候选词视口缓存及二级卡片展开交互状态；
///   3. [`UserAdaptiveState`]：用户个人词频模型与预写日志 (WAL) 持久化生命周期；
///   4. [`CompositionBuffer`]：增量击键编辑缓冲区、光标跟踪、分段累积与预编辑切分缓存。
#[derive(Debug, Default)]
pub struct AlgoEngine {
    /// 静态系统词典与语言学规则资源包
    pub resources: LexiconResources,
    /// 候选词展示视口与分页交互状态机
    pub viewport: CandidateViewport,
    /// 用户自适应词频学习与预写日志持久化子系统
    pub adaptive: UserAdaptiveState,
    /// 增量击键编辑缓冲区与预编辑切分状态机
    pub composition: CompositionBuffer,
    /// 全局算法配置
    pub config: AlgoConfig,
    /// 候选词收集复用工作区缓冲（预分配容量，避免高频击键时数十次 Vec 分配与重新扩容）
    pub(crate) scratch_candidates: Vec<AlgoCandidate>,
}

impl AlgoEngine {
    /// 载入外部辅助与混输二进制词库（包含英文词库与符号表情库）
    #[inline]
    pub fn load_auxiliary_dict(&mut self, dict: AuxiliaryDict) {
        self.resources.load_auxiliary_dict(dict);
    }

    /// 动态注册新的受保护专有名词与实体
    #[inline]
    pub fn register_entity(&mut self, entity: &str) {
        self.resources.register_entity(entity);
    }

    /// 使用指定词典表与配置创建算法引擎
    pub fn new(table: LexiconTable, config: AlgoConfig) -> Self {
        Self {
            resources: LexiconResources::new(table),
            viewport: CandidateViewport::new(),
            adaptive: UserAdaptiveState::new(),
            composition: CompositionBuffer::new(),
            config,
            scratch_candidates: Vec::with_capacity(128),
        }
    }

    /// 设置用户自适应词库持久化文件路径并自动重放加载已有 WAL 记录
    pub fn set_user_dict_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.adaptive.set_dict_path(path);
    }

    /// 强制执行用户自适应词库的归并与刷盘持久化 (WAL Compaction)
    pub fn persist_user_dict(&mut self) -> Result<(), AlgoError> {
        self.adaptive.compact()
    }

    /// 检查并在达到压缩阈值时对用户词库执行归并压缩（通常在会话失焦时调用）
    pub fn maybe_compact_user_dict(&mut self) -> Result<bool, AlgoError> {
        self.adaptive.maybe_compact()
    }

    /// 判断磁盘上的 WAL 文件是否与当前引擎持有的常驻追加句柄指向同一文件
    pub fn is_same_wal_file(&self, path: &Path) -> bool {
        self.adaptive.is_same_wal_file(path)
    }
}

impl Drop for AlgoEngine {
    fn drop(&mut self) {
        if let Some(ref path) = self.adaptive.dict_path {
            log::info!(
                "[算法引擎] 引擎正在析构，确保用户自适应词库落盘: {:?}",
                path
            );
            if let Err(e) = self.adaptive.compact() {
                log::warn!("[算法引擎] 析构保存用户词库失败: {}", e);
            } else {
                log::info!("[算法引擎] 析构已成功完成用户词库安全落盘。");
            }
        }
    }
}

impl AlgoEngine {
    /// 使用内置样例数据创建算法引擎（用于测试）
    pub fn with_sample_data(config: AlgoConfig) -> Self {
        Self::new(LexiconTable::with_sample_data(), config)
    }

    /// 键入一个字符
    pub fn push_char(&mut self, ch: char) -> &[AlgoCandidate] {
        self.composition.push_char(ch);
        self.viewport.page_index = 0;
        self.recompute_candidates();
        &self.viewport.current_candidates
    }

    /// 退格删除一个字符（修改输入时清空分段自造词累积器，防止拼装错误脏词）
    pub fn pop_char(&mut self) -> &[AlgoCandidate] {
        self.composition.pop_char();
        self.viewport.page_index = 0;
        self.recompute_candidates();
        &self.viewport.current_candidates
    }

    /// 向后删除光标右侧的一个字符（Delete 键行为，并清空分段自造词累积器）
    pub fn delete_char_at_cursor(&mut self) -> &[AlgoCandidate] {
        self.composition.delete_char_at_cursor();
        self.viewport.page_index = 0;
        self.recompute_candidates();
        &self.viewport.current_candidates
    }

    /// 光标左移
    pub fn move_cursor_left(&mut self) -> &[AlgoCandidate] {
        if self.composition.move_cursor_left() {
            self.viewport.page_index = 0;
            self.viewport.reset_selection();
            self.recompute_candidates();
        }
        &self.viewport.current_candidates
    }

    /// 光标右移
    pub fn move_cursor_right(&mut self) -> &[AlgoCandidate] {
        if self.composition.move_cursor_right() {
            self.viewport.page_index = 0;
            self.viewport.reset_selection();
            self.recompute_candidates();
        }
        &self.viewport.current_candidates
    }

    /// 候选翻下一页（若已处于末页则保持当前页不产生空白页）
    pub fn next_page(&mut self) -> &[AlgoCandidate] {
        self.viewport.next_page(self.config.page_size)
    }

    /// 候选翻上一页
    pub fn prev_page(&mut self) -> &[AlgoCandidate] {
        self.viewport.prev_page(self.config.page_size)
    }

    /// 检查是否能够向后（下一页）翻页
    pub fn can_next_page(&self) -> bool {
        self.viewport.can_next_page(self.config.page_size)
    }

    /// 检查是否能够向前（上一页）翻页
    pub fn can_prev_page(&self) -> bool {
        self.viewport.can_prev_page()
    }

    /// 获取当前展示页码（从 0 开始）
    #[inline]
    pub fn page_index(&self) -> usize {
        self.viewport.page_index
    }

    /// 获取当前页内高亮选中的候选词索引（从 0 开始）
    #[inline]
    pub fn selected_index(&self) -> usize {
        self.viewport.selected_index
    }

    /// 二级扩展卡片是否展开
    #[inline]
    pub fn is_expanded(&self) -> bool {
        self.viewport.is_expanded
    }

    /// 二级扩展卡片中当前选中的子项索引（从 0 开始）
    #[inline]
    pub fn expanded_selected_index(&self) -> usize {
        self.viewport.expanded_selected_index
    }

    /// 将拼音编辑光标重置回末尾并刷新候选列表（恢复选词模式）
    pub fn move_cursor_to_end(&mut self) -> &[AlgoCandidate] {
        if self.composition.move_to_end() {
            self.viewport.page_index = 0;
            self.viewport.reset_selection();
            self.recompute_candidates();
        }
        &self.viewport.current_candidates
    }

    /// 根据当前页码与页面大小从全量候选缓存池中截取当前展示页切片
    pub(crate) fn update_page_candidates(&mut self) {
        self.viewport.update_page_candidates(self.config.page_size);
    }

    /// 在当前页候选词中向后（向下/向右）循环移动高亮选中项
    pub fn next_candidate(&mut self) {
        self.viewport.next_candidate();
    }

    /// 在当前页候选词中向前（向上/向左）循环移动高亮选中项
    pub fn prev_candidate(&mut self) {
        self.viewport.prev_candidate();
    }

    /// 批量导入外部词库条目到用户自适应模型
    pub fn import_dict_entries(&mut self, entries: &[ImportedDictEntry]) {
        self.adaptive.import_dict_entries(entries);
    }

    /// 获取当前输入缓冲区内容
    pub fn raw_input(&self) -> &str {
        self.composition.as_str()
    }

    /// 获取带智能音节切分隔音符的格式化预编辑文本（直接借用已缓存切分字符串或按需生成，零堆分配）
    pub fn formatted_preedit_cow(&self) -> Cow<'_, str> {
        let raw = self.composition.as_str();
        if raw.is_empty() {
            return Cow::Borrowed("");
        }
        if !self.composition.cached_preedit().is_empty() {
            return Cow::Borrowed(self.composition.cached_preedit());
        }

        let paths = PrismEngine::build_paths_with_entities(
            raw,
            &self.config.fuzzy,
            Some(&self.resources.entity_manager),
        );
        if let Some(first_path) = paths.first() {
            return Cow::Owned(first_path.to_key_string());
        }

        Cow::Borrowed(raw)
    }

    /// 获取带智能音节切分隔音符的格式化预编辑文本（由音节语言学切分直接驱动，如 "wo'men", "shan'xi", "xian"）
    pub fn formatted_preedit(&self) -> String {
        self.formatted_preedit_cow().into_owned()
    }

    /// 计算当前光标在带隔音符的预编辑字符串中的精确 UTF-8 字节偏移（直接借用缓存切片，杜绝克隆）
    pub fn formatted_cursor_byte_offset(&self) -> usize {
        let raw = self.composition.as_str();
        let raw_cursor = self.composition.cursor().min(raw.len());
        if raw.is_empty() || raw_cursor == 0 {
            return 0;
        }

        let formatted = self.formatted_preedit_cow();
        if raw_cursor >= raw.len() {
            return formatted.len();
        }

        let raw_prefix = &raw[..raw_cursor];
        let mut raw_iter = raw_prefix.chars();
        let mut target_opt = raw_iter.next();
        let mut offset = 0;

        for (idx, ch) in formatted.char_indices() {
            if let Some(target) = target_opt {
                if ch.eq_ignore_ascii_case(&target) {
                    offset = idx + ch.len_utf8();
                    target_opt = raw_iter.next();
                } else if ch == '\'' && target != '\'' {
                    continue;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        if target_opt.is_none() {
            return offset.min(formatted.len());
        }

        // 前缀切分场景：基于前缀切分路径长度计算字节偏移
        let prefix_paths = PrismEngine::build_paths_with_entities(
            raw_prefix,
            &self.config.fuzzy,
            Some(&self.resources.entity_manager),
        );
        if let Some(first) = prefix_paths.first() {
            let key_len = first.to_key_string().len().min(formatted.len());
            if formatted.is_char_boundary(key_len) {
                return key_len;
            }
        }

        formatted.len()
    }

    /// 获取内部词典表不可变引用
    #[inline]
    pub fn table(&self) -> &LexiconTable {
        &self.resources.table
    }

    /// 获取内部词典表可变引用
    #[inline]
    pub fn table_mut(&mut self) -> &mut LexiconTable {
        &mut self.resources.table
    }

    /// 获取当前光标位置
    #[inline]
    pub fn cursor(&self) -> usize {
        self.composition.cursor()
    }

    /// 获取当前呈现的候选词切片
    #[inline]
    pub fn candidates(&self) -> &[AlgoCandidate] {
        self.viewport.candidates()
    }

    /// 获取当前输入下的全量候选词切片（用于展开大面板展示）
    #[inline]
    pub fn all_candidates(&self) -> &[AlgoCandidate] {
        self.viewport.all_candidates()
    }

    /// 清空上屏历史上下文缓存
    pub fn clear_commit_context(&mut self) {
        self.composition.clear_commit_context();
    }

    /// 清空并重置引擎状态机与候选词视口
    pub fn reset(&mut self) {
        self.composition.reset();
        self.viewport.reset();
    }

    /// 获取当前用户自适应词库持久化路径
    #[inline]
    pub fn user_dict_path(&self) -> Option<&Path> {
        self.adaptive.dict_path.as_deref()
    }

    /// 获取上一轮上屏历史上下文词条
    #[inline]
    pub fn last_commit_context(&self) -> Option<&str> {
        self.composition.last_commit_context()
    }
}

impl AlgoEngine {
    /// 检查两个相邻文本之间是否需要插入盘古之白（中西文智能空格）
    #[inline]
    pub fn should_insert_pangu_space(prev_context: &str, curr_commit: &str) -> bool {
        PanguSpacing::should_insert_pangu_space(prev_context, curr_commit)
    }

    /// 判断是否属于常见中文量词或单位
    #[inline]
    pub fn is_chinese_classifier(word: &str) -> bool {
        PanguSpacing::is_chinese_classifier(word)
    }
}
