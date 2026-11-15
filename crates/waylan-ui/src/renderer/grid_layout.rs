//! 5 行网格大候选词面板的几何布局与尺寸度量模块
//!
//! # 设计原理
//! - **实现初衷**：将网格行列尺寸测量、列起始横坐标及滚动条几何参数计算从 `grid.rs` 中剥离，
//!   使绘制逻辑与纯数学度量各自保持精简。
//! - **核心优势**：布局常量与尺寸计算集中收敛，便于后续独立单测或扩展网格列数。
//! - **代价与局限**：跨模块共享结构体字段需使用 `pub(super)` 可见性。

use super::CandidateBoxRenderer;
use waylan_core::Candidate;

/// 网格大面板布局常量
pub(super) const GRID_ROWS: usize = 5;
pub(super) const GRID_COLS: usize = 6;
pub(super) const MAX_GRID_CAPACITY: usize = GRID_ROWS * GRID_COLS;

/// 5 行网格尺寸与度量结果
pub(super) struct GridDimensions {
    pub(super) col_widths: [f32; GRID_COLS],
    pub(super) row_height: f32,
    pub(super) total_width: f32,
    pub(super) total_height: f32,
    pub(super) padding_h: f32,
    pub(super) padding_v: f32,
    pub(super) actual_rows: usize,
    pub(super) selected_row: usize,
    pub(super) s: f32,
}

/// 网格右侧滚动条几何参数
pub(super) struct GridScrollbarParams {
    pub(super) scroll_x: f32,
    pub(super) scroll_y: f32,
    pub(super) scroll_h: f32,
    pub(super) thumb_y: f32,
    pub(super) thumb_h: f32,
    pub(super) scale: f32,
}

/// 计算网格各列宽度与总宽高
pub(super) fn compute_grid_dimensions(
    renderer: &mut CandidateBoxRenderer,
    candidates: &[Candidate],
    selected_index: usize,
    s: f32,
) -> GridDimensions {
    let col_widths = measure_grid_col_widths(renderer, candidates, s);

    let padding_h = 12.0 * s;
    let padding_v = 10.0 * s;
    let row_height = 28.0 * s;
    let col_gap = 8.0 * s;
    let right_action_w = 26.0 * s;

    let cols_total: f32 =
        col_widths.iter().sum::<f32>() + (GRID_COLS.saturating_sub(1) as f32 * col_gap);
    let total_width = padding_h * 2.0 + cols_total + right_action_w;

    let count = candidates.len().min(MAX_GRID_CAPACITY);
    let actual_rows = count.div_ceil(GRID_COLS).clamp(1, GRID_ROWS);
    let total_height = padding_v * 2.0 + actual_rows as f32 * row_height;

    GridDimensions {
        col_widths,
        row_height,
        total_width,
        total_height,
        padding_h,
        padding_v,
        actual_rows,
        selected_row: selected_index / GRID_COLS,
        s,
    }
}

/// 逐项测量网格各列所需的最小宽度
fn measure_grid_col_widths(
    renderer: &mut CandidateBoxRenderer,
    candidates: &[Candidate],
    s: f32,
) -> [f32; GRID_COLS] {
    let font_size = renderer.font_size * s;
    let index_font_size = font_size * 0.85;
    let mut col_widths = [40.0 * s; GRID_COLS];

    for (idx, cand) in candidates.iter().take(MAX_GRID_CAPACITY).enumerate() {
        let col = idx % GRID_COLS;
        let text_w = renderer.measure_text(&cand.text, font_size);
        let num_str = renderer.index_label(col);
        let num_w = if num_str.is_empty() {
            0.0
        } else {
            renderer.measure_text(&num_str, index_font_size)
        };
        let item_w = num_w + 6.0 * s + text_w + 14.0 * s;
        if item_w > col_widths[col] {
            col_widths[col] = item_w;
        }
    }
    col_widths
}

/// 计算网格各列的起始横坐标
pub(super) fn compute_grid_col_starts(
    padding_h: f32,
    col_widths: &[f32; GRID_COLS],
    col_gap: f32,
) -> [f32; GRID_COLS] {
    let mut col_starts = [0.0; GRID_COLS];
    let mut acc_x = padding_h;
    for (c, start) in col_starts.iter_mut().enumerate() {
        *start = acc_x;
        acc_x += col_widths[c] + col_gap;
    }
    col_starts
}

/// 计算网格右侧滚动条滑块的几何参数
pub(super) fn compute_grid_scrollbar(
    dims: &GridDimensions,
    ctrl_top_y: f32,
    s: f32,
) -> GridScrollbarParams {
    let scroll_x = dims.total_width - 9.0 * s;
    let scroll_y = ctrl_top_y + 16.0 * s;
    let scroll_h = (dims.total_height - scroll_y - dims.padding_v).max(18.0 * s);
    let thumb_h = (scroll_h / (dims.actual_rows.max(1) as f32)).clamp(12.0 * s, scroll_h);
    let thumb_y = if dims.actual_rows > 1 {
        let ratio =
            dims.selected_row.min(dims.actual_rows - 1) as f32 / (dims.actual_rows - 1) as f32;
        scroll_y + (scroll_h - thumb_h) * ratio
    } else {
        scroll_y
    };
    GridScrollbarParams {
        scroll_x,
        scroll_y,
        scroll_h,
        thumb_y,
        thumb_h,
        scale: s,
    }
}
