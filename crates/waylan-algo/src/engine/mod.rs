pub mod candidates;
pub mod pangu;
pub(crate) mod prefix_synthesis;
pub mod resources;
pub mod selection;
pub mod typo;
pub mod viewport;

#[cfg(test)]
mod tests;

pub use pangu::PanguSpacing;
pub use resources::LexiconResources;
pub use viewport::CandidateViewport;

use std::borrow::Cow;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::composition::Composition;
use crate::dict_importer::ImportedDictEntry;
use crate::error::AlgoError;
use crate::prism::PrismEngine;
use crate::table::LexiconTable;
use crate::types::{AlgoCandidate, AlgoConfig};
use crate::user_adaptive::{UserAdaptiveModel, UserAdaptiveWal};

/// 微澜统一核心算法引擎门面 (AlgoEngine)
///
/// # 设计原理
/// - **实现初衷**：将模式分流、DAG 拼写图、词典倒排、Viterbi 解码、自适应学习与后处理流水线封装为高内聚门面，
///   向输入法上层提供极简的按键响应与选词接口。
/// - **核心优势**：完全纯 Rust 实现，零 C++ 依赖；全生命周期无锁、无异步竞态；100% 内存安全与可测试。
/// - **代价与局限**：门面结构体聚合了多个子模块状态，但在单进程输入法模型下保证了单一状态真实源（Single Source of Truth）。
#[derive(Debug, Default)]
pub struct AlgoEngine {
    /// 静态系统词典与语言学规则资源包
    pub resources: LexiconResources,
    /// 候选词展示视口与分页交互状态机
    pub viewport: CandidateViewport,
    /// 用户自适应词频纯内存模型
    pub user_model: UserAdaptiveModel,
    /// 用户词库预写日志持久化管理器
    pub wal: UserAdaptiveWal,
    /// 增量敲击编辑状态机
    pub(crate) composition: Composition,
    /// 全局算法配置
    pub config: AlgoConfig,
    /// 上一次选词上屏的历史上下文词条（用于跨句 Bigram 语言模型联想预测）
    pub last_commit_context: Option<String>,
    /// 用户自适应词库 WAL 持久化文件路径（若设置则每次选词自动追加写盘）
    pub user_dict_path: Option<PathBuf>,
    /// 单次连续拼音输入过程中的分段选词累积器 (词条文本, 拼音片段)，用于整串消费完毕时自动合成用户自造新词
    pub(crate) partial_commit_segments: Vec<(String, String)>,
    /// 最近一次交互/选词的时间戳（秒级 UNIX 时间，用于驱动词频半衰期衰减评估）
    pub(crate) last_timestamp: u64,
    /// 当前输入缓冲区经 Prism 切分后的格式化预编辑文本缓存（避免渲染与光标定位重复执行 DAG 搜索）
    pub(crate) cached_preedit: String,
    /// 候选词收集复用工作区缓冲（预分配容量，避免高频击键时数十次 Vec 分配与重新扩容）
    pub(crate) scratch_candidates: Vec<AlgoCandidate>,
}

impl Deref for AlgoEngine {
    type Target = LexiconResources;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.resources
    }
}

impl DerefMut for AlgoEngine {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.resources
    }
}

impl AlgoEngine {
    /// 使用指定词典表与配置创建算法引擎
    pub fn new(table: LexiconTable, config: AlgoConfig) -> Self {
        Self {
            resources: LexiconResources::new(table),
            viewport: CandidateViewport::new(),
            user_model: UserAdaptiveModel::new(),
            wal: UserAdaptiveWal::new(),
            composition: Composition::new(),
            config,
            last_commit_context: None,
            user_dict_path: None,
            partial_commit_segments: Vec::new(),
            last_timestamp: 0,
            cached_preedit: String::new(),
            scratch_candidates: Vec::with_capacity(128),
        }
    }

    /// 设置用户自适应词库持久化文件路径并自动重放加载已有 WAL 记录
    pub fn set_user_dict_path(&mut self, path: impl Into<PathBuf>) {
        let p = path.into();
        if p.exists() {
            if let Ok(model) = UserAdaptiveModel::load_or_replay_wal(&p) {
                self.user_model = model;
            }
        }
        self.user_dict_path = Some(p);
    }

    /// 强制执行用户自适应词库的归并与刷盘持久化 (WAL Compaction)
    pub fn persist_user_dict(&mut self) -> Result<(), AlgoError> {
        if let Some(ref path) = self.user_dict_path {
            self.wal.compact(&self.user_model, path)?;
        }
        Ok(())
    }

    /// 检查并在达到压缩阈值时对用户词库执行归并压缩（通常在会话失焦时调用）
    pub fn maybe_compact_user_dict(&mut self) -> Result<bool, AlgoError> {
        if let Some(ref path) = self.user_dict_path {
            self.wal.maybe_compact(&self.user_model, path)
        } else {
            Ok(false)
        }
    }

    /// 判断磁盘上的 WAL 文件是否与当前引擎持有的常驻追加句柄指向同一文件
    pub fn is_same_wal_file(&self, path: &Path) -> bool {
        self.wal.is_same_wal_file(path)
    }
}

impl Drop for AlgoEngine {
    fn drop(&mut self) {
        if let Some(ref path) = self.user_dict_path {
            log::info!(
                "[算法引擎] 引擎正在析构，确保用户自适应词库落盘: {:?}",
                path
            );
            if let Err(e) = self.wal.compact(&self.user_model, path) {
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
        // 若在光标非末尾处插入字符，说明破坏了原有连续分段拼音流，清空自造词累积器
        if self.composition.cursor() < self.composition.len() {
            self.partial_commit_segments.clear();
        }
        self.composition.push_char(ch);
        self.viewport.page_index = 0;
        self.recompute_candidates();
        &self.viewport.current_candidates
    }

    /// 退格删除一个字符（修改输入时清空分段自造词累积器，防止拼装错误脏词）
    pub fn pop_char(&mut self) -> &[AlgoCandidate] {
        self.composition.pop_char();
        self.partial_commit_segments.clear();
        self.viewport.page_index = 0;
        self.recompute_candidates();
        &self.viewport.current_candidates
    }

    /// 向后删除光标右侧的一个字符（Delete 键行为，并清空分段自造词累积器）
    pub fn delete_char_at_cursor(&mut self) -> &[AlgoCandidate] {
        self.composition.delete_char_at_cursor();
        self.partial_commit_segments.clear();
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
        let sys_now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1_700_000_000);
        let now = sys_now
            .max(self.user_model.latest_timestamp())
            .max(self.last_timestamp);
        self.last_timestamp = now;
        for entry in entries {
            self.user_model
                .record_commit(&entry.text, &entry.pinyin, now);
        }
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
        if !self.cached_preedit.is_empty() {
            return Cow::Borrowed(self.cached_preedit.as_str());
        }

        let paths = PrismEngine::build_paths_with_entities(
            raw,
            &self.config.fuzzy,
            Some(&self.entity_manager),
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
            Some(&self.entity_manager),
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
        self.last_commit_context = None;
    }

    /// 清空并重置引擎状态机与候选词视口
    pub fn reset(&mut self) {
        self.composition.clear();
        self.viewport.reset();
        self.last_commit_context = None;
        self.partial_commit_segments.clear();
        self.cached_preedit.clear();
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
