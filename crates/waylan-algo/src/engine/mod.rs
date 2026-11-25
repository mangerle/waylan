pub mod adaptive;
pub mod candidates;
pub mod composition_buffer;
pub(crate) mod emoji;
pub mod extension;
pub mod resources;
pub mod selection;

#[cfg(test)]
mod tests;

pub use adaptive::UserAdaptiveState;
pub use composition_buffer::CompositionBuffer;
pub use extension::{
    CandidateExtensionProvider, CustomPhraseProvider, DateTimeProvider, EnglishSuggesterProvider,
    ExtensionContext, ExtensionPipeline, SymbolSuggesterProvider,
};
pub use resources::LexiconResources;
pub use waylan_core::PanguSpacing;

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use crate::custom_phrase::AuxiliaryDict;
use crate::decoder::{IncrementalBeamCache, LatticeOptions, SyllableLattice};
use crate::error::AlgoError;
use crate::table::LexiconTable;
use crate::types::{AlgoCandidate, AlgoConfig, ImportedDictEntry};

/// 微澜统一核心算法引擎门面 (AlgoEngine)
///
/// # 设计原理
/// - **实现初衷**：将模式分流、DAG 拼写图、词典倒排、Viterbi 解码、自适应学习与后处理流水线封装为高内聚门面，
///   向输入法上层提供极简的按键响应与选词接口。
/// - **核心优势**：完全纯 Rust 实现，零 C++ 依赖；全生命周期无锁、无异步竞态；100% 内存安全与可测试。
/// - **解构架构**：解构为高内聚的三大子系统：
///   1. [`LexiconResources`]：只读系统词典、实体保护、自定义短语及简繁字典资源；
///   2. [`UserAdaptiveState`]：用户个人词频模型与预写日志 (WAL) 持久化生命周期；
///   3. [`CompositionBuffer`]：增量击键编辑缓冲区、光标跟踪、分段累积与预编辑切分缓存。
#[derive(Debug, Default)]
pub struct AlgoEngine {
    /// 静态系统词典与语言学规则资源包
    pub(crate) resources: LexiconResources,
    /// 全量候选词计算结果缓存池（由算法解码计算产出，由上层会话层负责分页视口呈现）
    pub(crate) candidates: Vec<AlgoCandidate>,
    /// 用户自适应词频学习与预写日志持久化子系统
    pub(crate) adaptive: UserAdaptiveState,
    /// 增量击键编辑缓冲区与预编辑切分状态机
    pub(crate) composition: CompositionBuffer,
    /// 全局算法配置
    pub config: AlgoConfig,
    /// 候选词收集复用工作区缓冲（预分配容量，避免高频击键时数十次 Vec 分配与重新扩容）
    pub(crate) scratch_candidates: Vec<AlgoCandidate>,
    /// 按字节前缀冻结的增量束搜索解码缓存
    pub(crate) beam_cache: IncrementalBeamCache,
    /// 已完成延迟后处理（拼音注释与消费长度标记）的候选前缀长度
    pub(crate) prepared_up_to: usize,
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
            candidates: Vec::new(),
            adaptive: UserAdaptiveState::new(),
            composition: CompositionBuffer::new(),
            config,
            scratch_candidates: Vec::with_capacity(128),
            beam_cache: IncrementalBeamCache::new(),
            prepared_up_to: 0,
        }
    }

    /// 设置用户自适应词库持久化文件路径并自动重放加载已有 WAL 记录
    pub fn set_user_dict_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.adaptive.set_dict_path(path);
        self.beam_cache.invalidate();
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
        self.recompute_candidates();
        &self.candidates
    }

    /// 退格删除一个字符（修改输入时清空分段自造词累积器，防止拼装错误脏词）
    pub fn pop_char(&mut self) -> &[AlgoCandidate] {
        self.composition.pop_char();
        self.recompute_candidates();
        &self.candidates
    }

    /// 向后删除光标右侧的一个字符（Delete 键行为，并清空分段自造词累积器）
    pub fn delete_char_at_cursor(&mut self) -> &[AlgoCandidate] {
        self.composition.delete_char_at_cursor();
        self.recompute_candidates();
        &self.candidates
    }

    /// 光标左移
    pub fn move_cursor_left(&mut self) -> &[AlgoCandidate] {
        if self.composition.move_cursor_left() {
            self.recompute_candidates();
        }
        &self.candidates
    }

    /// 光标右移
    pub fn move_cursor_right(&mut self) -> &[AlgoCandidate] {
        if self.composition.move_cursor_right() {
            self.recompute_candidates();
        }
        &self.candidates
    }

    /// 将拼音编辑光标重置回末尾并刷新候选列表（恢复选词模式）
    pub fn move_cursor_to_end(&mut self) -> &[AlgoCandidate] {
        if self.composition.move_to_end() {
            self.recompute_candidates();
        }
        &self.candidates
    }

    /// 获取用户自适应模型中已记录的历史词条总数
    #[inline]
    pub fn user_adaptive_total_entries(&self) -> usize {
        self.adaptive.model.total_entries()
    }

    /// 获取用户词库归并自写入时间戳追踪器（供外部 file watcher 抑制自身写盘触发的热重载）
    #[inline]
    pub fn user_dict_compact_tracker(&self) -> Arc<AtomicU64> {
        self.adaptive.last_compact_time()
    }

    /// 重置用户自适应学习模型
    #[inline]
    pub fn reset_user_adaptive_model(&mut self) {
        self.adaptive.model = crate::user_adaptive::UserAdaptiveModel::new();
        self.beam_cache.invalidate();
    }

    /// 批量导入外部词库条目到用户自适应模型
    pub fn import_dict_entries(&mut self, entries: &[ImportedDictEntry]) {
        self.adaptive.import_dict_entries(entries);
        self.beam_cache.invalidate();
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

        let lattice = SyllableLattice::build(
            raw,
            &LatticeOptions {
                fuzzy: self.config.fuzzy,
                typo_rules: self.config.typo_rules,
                enable_mixed_jianpin: self.config.enable_acronym,
                enable_typo_edges: self.config.typo_rules.enable,
            },
        );
        Cow::Owned(lattice.best_formatted_preedit())
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
        match Self::scan_formatted_offset(&formatted, raw_prefix) {
            Some(offset) => offset,
            // 前缀切分场景：基于前缀切分路径长度计算字节偏移
            None => self.prefix_path_byte_offset(raw_prefix, &formatted),
        }
    }

    /// 逐字符对齐原始输入前缀与格式化预编辑文本，全部前缀字符命中时返回对应字节偏移
    fn scan_formatted_offset(formatted: &str, raw_prefix: &str) -> Option<usize> {
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
            Some(offset.min(formatted.len()))
        } else {
            None
        }
    }

    /// 基于前缀音节格最优格式化长度计算光标字节偏移（逐字符对齐无法命中时的回退策略）
    fn prefix_path_byte_offset(&self, raw_prefix: &str, formatted: &str) -> usize {
        let lattice = SyllableLattice::build(
            raw_prefix,
            &LatticeOptions {
                fuzzy: self.config.fuzzy,
                typo_rules: self.config.typo_rules,
                enable_mixed_jianpin: self.config.enable_acronym,
                enable_typo_edges: self.config.typo_rules.enable,
            },
        );
        let key_len = lattice.best_formatted_preedit().len().min(formatted.len());
        if formatted.is_char_boundary(key_len) {
            return key_len;
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

    /// 获取静态词典与规则资源包不可变引用
    #[inline]
    pub fn resources(&self) -> &LexiconResources {
        &self.resources
    }

    /// 获取静态词典与规则资源包可变引用
    #[inline]
    pub fn resources_mut(&mut self) -> &mut LexiconResources {
        &mut self.resources
    }

    /// 获取当前光标位置
    #[inline]
    pub fn cursor(&self) -> usize {
        self.composition.cursor()
    }

    /// 获取当前输入下计算得到的全量候选词切片
    #[inline]
    pub fn candidates(&self) -> &[AlgoCandidate] {
        &self.candidates
    }

    /// 获取当前输入下计算得到的全量候选词切片（用于展开大面板展示或与 candidates 保持一致）
    #[inline]
    pub fn all_candidates(&self) -> &[AlgoCandidate] {
        &self.candidates
    }

    /// 清空上屏历史上下文缓存
    pub fn clear_commit_context(&mut self) {
        self.composition.clear_commit_context();
    }

    /// 清空并重置引擎状态机与候选词缓存
    pub fn reset(&mut self) {
        self.composition.reset();
        self.candidates.clear();
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
