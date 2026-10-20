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
    ///
    /// # 设计原理
    /// - 判定顺序固定为：方向键与 Esc 折叠导航 -> 数字键 1..=6 选词 -> 空格键确认选词，
    ///   三级拦截严格互斥，任何一级命中后立即返回，不得改变既有优先级。
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

        let layout = derive_grid_layout(total, selected_index);

        // 1. 方向键与 Esc 折叠导航
        if let Some(target) = grid_navigation_target(&layout, key, selected_index) {
            selected_index = target;
            self.expansion = CandidateExpansion::Grid { selected_index };
            return Some(ImeOutputAction::Handled { update_ui: true });
        }
        if key == KEY_ESC {
            self.collapse_grid();
            return Some(ImeOutputAction::Handled { update_ui: true });
        }

        // 2. 数字键 1..=6 选词（Shift 状态下需显式开启 Shift 选词）
        if let Some(action) = self.handle_grid_digit_selection(key, &layout, now) {
            return Some(action);
        }

        // 3. 空格键确认选词
        if key == KEY_SPACE {
            let idx = selected_index.min(total.saturating_sub(1));
            return Some(self.commit_grid_candidate_by_index(idx, now));
        }

        None
    }

    /// 处理网格面板的数字键选词，返回 None 表示当前按键不属于数字选词分支
    ///
    /// # 设计原理
    /// - 仅在"未按下 Shift"或"已显式开启 Shift 选词"时才会拦截数字键；
    ///   命中数字键但目标行内并无候选词时，统一以"已消费但不刷新界面"的语义收口，
    ///   避免数字键被透传为原生日字符。
    fn handle_grid_digit_selection(
        &mut self,
        key: u32,
        layout: &GridLayout,
        now: u64,
    ) -> Option<ImeOutputAction> {
        if self.is_shift_active() && !self.shift_select_second {
            return None;
        }
        let digit = keycode_to_digit(key)?;
        if (1..=6).contains(&digit) {
            let target_idx = layout.row * GRID_COLS + (digit - 1);
            if target_idx < layout.total {
                return Some(self.commit_grid_candidate_by_index(target_idx, now));
            }
        }
        Some(ImeOutputAction::Handled { update_ui: false })
    }
}

/// 网格候选面板的几何布局参数
///
/// 由参与布局的候选总数与当前选中索引推导，用于方向键跨行导航与数字键行内定位。
struct GridLayout {
    /// 参与布局的候选总数
    total: usize,
    /// 网格行数（固定 6 列，最多 5 行）
    row_count: usize,
    /// 当前选中项所在行
    row: usize,
    /// 当前选中项所在列
    col: usize,
}

/// 依据候选总数与当前选中索引推导网格几何布局
fn derive_grid_layout(total: usize, selected_index: usize) -> GridLayout {
    GridLayout {
        total,
        row_count: total.div_ceil(GRID_COLS).clamp(1, 5),
        row: selected_index / GRID_COLS,
        col: selected_index % GRID_COLS,
    }
}

/// 计算方向键导航后的目标选中索引，返回 None 表示该按键不属于方向键导航
///
/// # 设计原理
/// - 上下方向键在行内保持列号不变并在首尾行循环，跨行后需按候选总数收敛边界；
///   左右方向键则在当前页候选序列内循环。
fn grid_navigation_target(layout: &GridLayout, key: u32, selected_index: usize) -> Option<usize> {
    let target = match key {
        KEY_DOWN => {
            let next_row = (layout.row + 1) % layout.row_count;
            (next_row * GRID_COLS + layout.col).min(layout.total.saturating_sub(1))
        }
        KEY_UP => {
            let prev_row = if layout.row == 0 {
                layout.row_count.saturating_sub(1)
            } else {
                layout.row - 1
            };
            (prev_row * GRID_COLS + layout.col).min(layout.total.saturating_sub(1))
        }
        KEY_RIGHT => (selected_index + 1) % layout.total,
        KEY_LEFT => {
            if selected_index == 0 {
                layout.total.saturating_sub(1)
            } else {
                selected_index - 1
            }
        }
        _ => return None,
    };
    Some(target)
}
