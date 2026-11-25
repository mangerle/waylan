//! 输入法核心会话候选词视口切片与展开卡片状态管理
//!
//! # 设计原理
//! - **实现初衷**：将候选词分页切片、光标位移、二级卡片与大网格交互状态从巨型 `mod.rs` 中解耦。
//! - **核心优势**：职责单一（SRP），视口导航与状态机变更集中内聚，杜绝越界越权访问。
//! - **代价与局限**：基于 `&[Candidate]` 切片返回借用，生命周期与会话自身绑定。

use waylan_core::{Candidate, CandidateExpansion};

use super::{ImeCoreSession, ImeOutputAction};

impl ImeCoreSession {
    /// 查询当前是否处于大候选网格展开态
    #[inline]
    pub fn is_grid_expanded(&self) -> bool {
        self.expansion.is_grid_expanded()
    }

    /// 查询当前是否处于二级扩展卡片展开态
    #[inline]
    pub fn is_card_expanded(&self) -> bool {
        self.expansion.is_card_expanded()
    }

    /// 展开二级格式扩展卡片（严格互斥：在类型上自动收起大网格面板）
    pub fn expand_card(&mut self, selected_sub_index: usize) {
        self.expansion = CandidateExpansion::Card {
            selected_index: selected_sub_index,
        };
    }

    /// 折叠二级格式扩展卡片
    #[inline]
    pub fn collapse_card(&mut self) {
        if self.expansion.is_card_expanded() {
            self.expansion = CandidateExpansion::None;
        }
    }

    /// 获取二级卡片展开态下的高亮子项索引，未展开则返回 0
    #[inline]
    pub fn card_selected_index(&self) -> usize {
        match self.expansion {
            CandidateExpansion::Card { selected_index } => selected_index,
            _ => 0,
        }
    }

    /// 获取当前展示页码（从 0 开始）
    #[inline]
    pub fn page_index(&self) -> usize {
        self.page_index
    }

    /// 获取当前页内高亮选中的候选词索引（从 0 开始）
    #[inline]
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// 设置当前展示页内的高亮候选词索引（自动边界保护）
    pub fn set_selected_index(&mut self, index: usize) {
        let max_idx = self.current_page_candidates().len().saturating_sub(1);
        self.selected_index = index.min(max_idx);
    }

    /// 获取当前候选框分页容量
    #[inline]
    pub fn page_size(&self) -> usize {
        self.page_size.max(1)
    }

    /// 设置当前候选框分页容量
    pub fn set_page_size(&mut self, size: usize) {
        self.page_size = size.max(1);
        self.clamp_viewport();
    }

    /// 将页码与页内选中索引钳制到当前候选列表的合法范围
    ///
    /// 候选数量变化（热重载分页容量、候选过滤或清空）后必须调用，否则 UI 可能停留在空页。
    pub fn clamp_viewport(&mut self) {
        let total = self.engine.candidates().len();
        let max_page = if total == 0 {
            0
        } else {
            total.saturating_sub(1) / self.page_size()
        };
        self.page_index = self.page_index.min(max_page);
        let max_idx = self.current_page_candidates().len().saturating_sub(1);
        self.selected_index = self.selected_index.min(max_idx);
    }

    /// 获取当前展示页切片的候选词列表
    pub fn current_page_candidates(&self) -> &[Candidate] {
        let all = self.engine.candidates();
        let size = self.page_size();
        let start = self.page_index.saturating_mul(size);
        if start < all.len() {
            let end = (start + size).min(all.len());
            &all[start..end]
        } else {
            &[]
        }
    }

    /// 检查是否能够向后（下一页）翻页
    pub fn can_next_page(&self) -> bool {
        let size = self.page_size();
        let max_page = self.engine.candidates().len().saturating_sub(1) / size;
        self.page_index < max_page
    }

    /// 检查是否能够向前（上一页）翻页
    pub fn can_prev_page(&self) -> bool {
        self.page_index > 0
    }

    /// 候选词翻下一页
    pub fn next_page(&mut self) -> &[Candidate] {
        if self.can_next_page() {
            self.page_index += 1;
            let end = (self.page_index + 1).saturating_mul(self.page_size().max(1));
            self.engine.prepare_candidates_up_to(end);
            self.selected_index = 0;
            self.collapse_card();
        }
        self.current_page_candidates()
    }

    /// 候选词翻上一页
    pub fn prev_page(&mut self) -> &[Candidate] {
        if self.can_prev_page() {
            self.page_index -= 1;
            self.selected_index = 0;
            self.collapse_card();
        }
        self.current_page_candidates()
    }

    /// 在当前展示页中向后（向下/向右）循环移动高亮选中项
    pub fn next_candidate(&mut self) {
        let len = self.current_page_candidates().len();
        if len > 0 {
            self.selected_index = (self.selected_index + 1) % len;
            self.collapse_card();
        }
    }

    /// 在当前展示页中向前（向上/向左）循环移动高亮选中项
    pub fn prev_candidate(&mut self) {
        let len = self.current_page_candidates().len();
        if len > 0 {
            self.selected_index = if self.selected_index == 0 {
                len.saturating_sub(1)
            } else {
                self.selected_index - 1
            };
            self.collapse_card();
        }
    }

    /// 计算当前高亮候选词在全量候选池中的绝对全局索引
    #[inline]
    pub fn selected_global_index(&self) -> usize {
        self.page_index.saturating_mul(self.page_size()) + self.selected_index
    }

    /// 提交指定全局索引的候选词上屏并更新会话视口状态机
    pub fn commit_candidate_by_global_index(
        &mut self,
        target_idx: usize,
        now: u64,
    ) -> ImeOutputAction {
        if let Some((committed_text, remaining_len)) = self.engine.select_candidate(target_idx, now)
        {
            self.reset_viewport();
            self.punctuation_processor.record_commit(&committed_text);
            if remaining_len > 0 {
                let remaining_preedit = self.engine.formatted_preedit();
                ImeOutputAction::CommitPartial {
                    committed: committed_text,
                    remaining_preedit,
                }
            } else {
                ImeOutputAction::Commit(committed_text)
            }
        } else {
            ImeOutputAction::Handled { update_ui: false }
        }
    }

    /// 提交二级展开卡片中指定索引的子词条并同步收起卡片、重置视口与登记标点上下文
    pub(crate) fn commit_card_sub_candidate(&mut self, sub_idx: usize) -> ImeOutputAction {
        let sel = self.selected_global_index();
        self.collapse_card();

        if let Some(cand) = self.engine.candidates().get(sel)
            && let Some(sub_text) = cand.sub_candidates.get(sub_idx).cloned()
        {
            let (committed, _) = self.engine.commit_custom_text(&sub_text);
            self.reset_viewport();
            self.punctuation_processor.record_commit(&committed);
            return ImeOutputAction::Commit(committed);
        }
        ImeOutputAction::Handled { update_ui: true }
    }

    /// 重置候选词视口状态（翻页归零、光标归零、关闭展开卡片与网格）
    pub fn reset_viewport(&mut self) {
        self.page_index = 0;
        self.selected_index = 0;
        self.expansion = CandidateExpansion::None;
    }

    /// 在二级展开卡片中向下移动选中项
    pub fn next_card_item(&mut self) {
        let sel = self.selected_global_index();
        let sub_len = self
            .engine
            .candidates()
            .get(sel)
            .map(|c| c.sub_candidates.len())
            .unwrap_or(0);
        if sub_len > 0
            && let CandidateExpansion::Card {
                ref mut selected_index,
            } = self.expansion
        {
            *selected_index = (*selected_index + 1) % sub_len;
        }
    }

    /// 在二级展开卡片中向上移动选中项
    pub fn prev_card_item(&mut self) {
        let sel = self.selected_global_index();
        let sub_len = self
            .engine
            .candidates()
            .get(sel)
            .map(|c| c.sub_candidates.len())
            .unwrap_or(0);
        if sub_len > 0
            && let CandidateExpansion::Card {
                ref mut selected_index,
            } = self.expansion
        {
            *selected_index = if *selected_index == 0 {
                sub_len.saturating_sub(1)
            } else {
                *selected_index - 1
            };
        }
    }

    /// 获取大候选网格展开态下的高亮选中项绝对索引，未展开则返回 0
    #[inline]
    pub fn grid_selected_index(&self) -> usize {
        match self.expansion {
            CandidateExpansion::Grid { selected_index } => selected_index,
            _ => 0,
        }
    }

    /// 设置大候选网格展开态下的选中索引
    #[inline]
    pub fn set_grid_selected_index(&mut self, idx: usize) {
        if self.expansion.is_grid_expanded() {
            self.expansion = CandidateExpansion::Grid {
                selected_index: idx,
            };
        }
    }
}
