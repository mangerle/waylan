//! 候选词展示视口与分页展开状态
//!
//! # 设计原理
//! - **实现初衷**：将候选词分页切片、高亮选框、二级扩展卡片及翻页逻辑从 AlgoEngine 主状态机中抽离，
//!   聚焦于候选词在界面的“视口呈现与导航交互”职责。
//! - **核心优势**：职责单一内聚，避免 AlgoEngine 成为臃肿的上帝对象；视口状态与算法解码计算彻底解耦。
//! - **代价与局限**：翻页更新时需要外部传入分页容量参数（page_size）。

use crate::types::AlgoCandidate;

/// 候选词展示视口与分页交互状态机
#[derive(Debug, Clone, Default)]
pub struct CandidateViewport {
    /// 当前候选词分页索引（0-indexed）
    pub page_index: usize,
    /// 当前页内高亮选中的候选词索引（0-indexed）
    pub selected_index: usize,
    /// 当前呈现的清洗后候选词缓存
    pub current_candidates: Vec<AlgoCandidate>,
    /// 全量候选词清洗与排序缓存池（翻页时直接切片，杜绝重复触发 Viterbi 与 DAG 重建）
    pub all_candidates_cache: Vec<AlgoCandidate>,
    /// 二级扩展卡片是否展开
    pub is_expanded: bool,
    /// 二级扩展卡片中当前选中的子项索引（0-indexed）
    pub expanded_selected_index: usize,
}

impl CandidateViewport {
    /// 创建全新的候选词展示视口
    pub fn new() -> Self {
        Self::default()
    }

    /// 重置视口状态与所有候选词缓存
    pub fn reset(&mut self) {
        self.page_index = 0;
        self.selected_index = 0;
        self.is_expanded = false;
        self.expanded_selected_index = 0;
        self.current_candidates.clear();
        self.all_candidates_cache.clear();
    }

    /// 重置选词高亮光标与扩展卡片展开状态
    pub fn reset_selection(&mut self) {
        self.selected_index = 0;
        self.is_expanded = false;
        self.expanded_selected_index = 0;
    }

    /// 根据当前页码与页面大小从全量候选缓存池中截取当前展示页切片
    pub fn update_page_candidates(&mut self, page_size: usize) {
        let size = page_size.max(1);
        let start = self.page_index.saturating_mul(size);
        if start < self.all_candidates_cache.len() {
            let end = (start + size).min(self.all_candidates_cache.len());
            self.current_candidates = self.all_candidates_cache[start..end].to_vec();
        } else {
            self.current_candidates.clear();
        }
    }

    /// 候选翻下一页（若已处于末页则保持当前页不产生空白页）
    pub fn next_page(&mut self, page_size: usize) -> &[AlgoCandidate] {
        let size = page_size.max(1);
        let max_page = self.all_candidates_cache.len().saturating_sub(1) / size;
        if self.page_index < max_page {
            self.page_index += 1;
            self.reset_selection();
            self.update_page_candidates(size);
        }
        &self.current_candidates
    }

    /// 候选翻上一页
    pub fn prev_page(&mut self, page_size: usize) -> &[AlgoCandidate] {
        let size = page_size.max(1);
        if self.page_index > 0 {
            self.page_index -= 1;
            self.reset_selection();
            self.update_page_candidates(size);
        }
        &self.current_candidates
    }

    /// 检查是否能够向后（下一页）翻页
    pub fn can_next_page(&self, page_size: usize) -> bool {
        let size = page_size.max(1);
        let max_page = self.all_candidates_cache.len().saturating_sub(1) / size;
        self.page_index < max_page
    }

    /// 检查是否能够向前（上一页）翻页
    pub fn can_prev_page(&self) -> bool {
        self.page_index > 0
    }

    /// 在当前页候选词中向后（向下/向右）循环移动高亮选中项
    pub fn next_candidate(&mut self) {
        if !self.current_candidates.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.current_candidates.len();
            self.is_expanded = false;
            self.expanded_selected_index = 0;
        }
    }

    /// 在当前页候选词中向前（向上/向左）循环移动高亮选中项
    pub fn prev_candidate(&mut self) {
        if !self.current_candidates.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = self.current_candidates.len().saturating_sub(1);
            } else {
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            self.is_expanded = false;
            self.expanded_selected_index = 0;
        }
    }

    /// 切换二级扩展卡片展开/收起状态
    pub fn toggle_expand(&mut self, selected_candidate_index: usize) -> bool {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if cand.is_expandable && !cand.sub_candidates.is_empty() {
                self.is_expanded = !self.is_expanded;
                self.expanded_selected_index = 0;
                return self.is_expanded;
            }
        }
        if !self.is_expanded {
            if let Some((idx, _)) = self
                .current_candidates
                .iter()
                .enumerate()
                .find(|(_, c)| c.is_expandable && !c.sub_candidates.is_empty())
            {
                self.selected_index = idx;
                self.is_expanded = true;
                self.expanded_selected_index = 0;
                return true;
            }
        }
        self.is_expanded = false;
        false
    }

    /// 在二级展开卡片中向下移动选中项
    pub fn next_expanded_item(&mut self, selected_candidate_index: usize) {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if !cand.sub_candidates.is_empty() {
                self.expanded_selected_index =
                    (self.expanded_selected_index + 1) % cand.sub_candidates.len();
            }
        }
    }

    /// 在二级展开卡片中向上移动选中项
    pub fn prev_expanded_item(&mut self, selected_candidate_index: usize) {
        if let Some(cand) = self.current_candidates.get(selected_candidate_index) {
            if !cand.sub_candidates.is_empty() {
                if self.expanded_selected_index == 0 {
                    self.expanded_selected_index = cand.sub_candidates.len().saturating_sub(1);
                } else {
                    self.expanded_selected_index = self.expanded_selected_index.saturating_sub(1);
                }
            }
        }
    }

    /// 获取当前呈现的候选词切片
    #[inline]
    pub fn candidates(&self) -> &[AlgoCandidate] {
        &self.current_candidates
    }

    /// 获取当前输入下的全量候选词切片（用于展开大面板展示）
    #[inline]
    pub fn all_candidates(&self) -> &[AlgoCandidate] {
        &self.all_candidates_cache
    }
}
