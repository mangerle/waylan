//! 核心输入法会话 5 行大候选面板网格导航与选词逻辑
//!
//! # 设计原理
//! - **实现初衷**：将大网格候选面板（5 行 6 列，共最多 30 个候选词）的键盘方向导航、
//!   数字选词与空格上屏收敛至核心会话层，消除协议端独立维护网格状态带来的逻辑割裂。
//! - **核心优势**：跨协议（Wayland V1、V2、IBus 等）统一的大网格交互体验，单点维护，零行为漂移。
//! - **代价与局限**：网格几何布局固定为 6 列、最多 5 行（30 个词）。

use super::{ImeCoreSession, ImeOutputAction};
use waylan_core::input::*;
use waylan_core::CandidateExpansion;

const GRID_COLS: usize = 6;
const MAX_GRID_CANDIDATES: usize = 30;

impl ImeCoreSession {
    /// 展开 5 行网格大候选词面板（严格互斥：在类型上自动收起二级卡片）
    pub fn expand_grid(&mut self) {
        self.expansion = CandidateExpansion::Grid {
            selected_index: self
                .selected_global_index()
                .min(MAX_GRID_CANDIDATES.saturating_sub(1)),
        };
    }

    /// 折叠 5 行网格大候选词面板并重置选中状态机
    pub fn collapse_grid(&mut self) {
        if self.expansion.is_grid_expanded() {
            self.expansion = CandidateExpansion::None;
        }
    }

    /// 切换 5 行网格大候选词面板的展开/折叠状态
    pub fn toggle_grid_expansion(&mut self) {
        if self.is_grid_expanded() {
            self.collapse_grid();
        } else {
            self.expand_grid();
        }
    }

    /// 提交指定全局索引的网格候选词上屏并退出大网格展开态
    fn commit_grid_candidate_by_index(&mut self, target_idx: usize, now: u64) -> ImeOutputAction {
        self.commit_candidate_by_global_index(target_idx, now)
    }

    /// 处理处于 5 行网格大候选词面板展开态下的专属键盘导航与选词
    pub(crate) fn handle_grid_expanded_key(
        &mut self,
        key: u32,
        now: u64,
    ) -> Option<ImeOutputAction> {
        let CandidateExpansion::Grid { mut selected_index } = self.expansion else {
            return None;
        };

        let total = self.engine.all_candidates().len().min(MAX_GRID_CANDIDATES);
        if total == 0 {
            return None;
        }

        let row_count = total.div_ceil(GRID_COLS).clamp(1, 5);
        let row = selected_index / GRID_COLS;
        let col = selected_index % GRID_COLS;

        // 1. 方向键与 Esc 折叠导航
        match key {
            KEY_DOWN => {
                let next_row = (row + 1) % row_count;
                selected_index = (next_row * GRID_COLS + col).min(total.saturating_sub(1));
                self.expansion = CandidateExpansion::Grid { selected_index };
                return Some(ImeOutputAction::Handled { update_ui: true });
            }
            KEY_UP => {
                let prev_row = if row == 0 {
                    row_count.saturating_sub(1)
                } else {
                    row - 1
                };
                selected_index = (prev_row * GRID_COLS + col).min(total.saturating_sub(1));
                self.expansion = CandidateExpansion::Grid { selected_index };
                return Some(ImeOutputAction::Handled { update_ui: true });
            }
            KEY_RIGHT => {
                selected_index = (selected_index + 1) % total;
                self.expansion = CandidateExpansion::Grid { selected_index };
                return Some(ImeOutputAction::Handled { update_ui: true });
            }
            KEY_LEFT => {
                selected_index = if selected_index == 0 {
                    total.saturating_sub(1)
                } else {
                    selected_index - 1
                };
                self.expansion = CandidateExpansion::Grid { selected_index };
                return Some(ImeOutputAction::Handled { update_ui: true });
            }
            KEY_ESC => {
                self.collapse_grid();
                return Some(ImeOutputAction::Handled { update_ui: true });
            }
            _ => {}
        }

        // 2. 数字键 1..=6 选词（Shift 状态下需显式开启 Shift 选词）
        if !self.is_shift_active() || self.shift_select_second {
            if let Some(digit) = keycode_to_digit(key) {
                if (1..=6).contains(&digit) {
                    let target_idx = row * GRID_COLS + (digit - 1);
                    if target_idx < total {
                        return Some(self.commit_grid_candidate_by_index(target_idx, now));
                    }
                }
                return Some(ImeOutputAction::Handled { update_ui: false });
            }
        }

        // 3. 空格键确认选词
        if key == KEY_SPACE {
            let idx = selected_index.min(total.saturating_sub(1));
            return Some(self.commit_grid_candidate_by_index(idx, now));
        }

        None
    }
}
