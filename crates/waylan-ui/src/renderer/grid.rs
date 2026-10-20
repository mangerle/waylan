//! 5 行网格大候选词面板渲染与布局模块
//!
//! 负责大面板展开状态下的 5 行 × 6 列网格候选词、右侧垂直滚动条、
//! 向上折叠箭头按钮的矢量绘制以及鼠标可点击区域计算。

use super::draw::{
    draw_card_background_at, draw_collapse_arrow, draw_rounded_rect, draw_scrollbar,
    CardBackgroundParams, RoundedRectParams, ScrollbarParams,
};
use super::layout::{CandidateHitTarget, HitRegion};
use super::{CandidateBoxRenderer, DrawTextOptions};
use tiny_skia::Pixmap;
use waylan_core::Candidate;

/// 网格大面板布局常量
const GRID_ROWS: usize = 5;
const GRID_COLS: usize = 6;
const MAX_GRID_CAPACITY: usize = GRID_ROWS * GRID_COLS;

/// 5 行网格尺寸与度量结果
struct GridDimensions {
    col_widths: [f32; GRID_COLS],
    row_height: f32,
    total_width: f32,
    total_height: f32,
    padding_h: f32,
    padding_v: f32,
    actual_rows: usize,
    selected_row: usize,
    s: f32,
}

/// 计算网格各列宽度与总宽高
fn compute_grid_dimensions(
    renderer: &mut CandidateBoxRenderer,
    candidates: &[Candidate],
    selected_index: usize,
    s: f32,
) -> GridDimensions {
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

/// 渲染 5 行展开大候选词网格面板
pub fn render_grid_expanded_panel(
    renderer: &mut CandidateBoxRenderer,
    candidates: &[Candidate],
    selected_index: usize,
    regions: &mut Vec<HitRegion>,
) -> Option<Pixmap> {
    let s = renderer.scale.max(0.5);
    if candidates.is_empty() {
        return None;
    }

    let dims = compute_grid_dimensions(renderer, candidates, selected_index, s);
    let align = (s.round() as u32).max(1);
    let clamped_w = dims.total_width.ceil().clamp(1.0, 4096.0) as u32;
    let clamped_h = dims.total_height.ceil().clamp(1.0, 2048.0) as u32;
    let canvas_w = clamped_w.div_ceil(align).saturating_mul(align);
    let canvas_h = clamped_h.div_ceil(align).saturating_mul(align);
    let mut pixmap = Pixmap::new(canvas_w, canvas_h)?;

    // 绘制大面板圆角卡片背景
    draw_card_background_at(
        &mut pixmap,
        &renderer.theme,
        CardBackgroundParams {
            x: 0.0,
            y: 0.0,
            width: canvas_w as f32,
            height: canvas_h as f32,
            radius: renderer.theme.corner_radius.max(6.0) * s,
            stroke_width: 1.0 * s,
        },
    );

    render_grid_items(
        renderer,
        &mut pixmap,
        GridRenderContext {
            selected_index,
            candidates,
            dims: &dims,
            regions,
        },
    );
    render_grid_controls(renderer, &mut pixmap, &dims, regions);

    Some(pixmap)
}

/// 网格项渲染上下文参数对象
struct GridRenderContext<'a, 'b> {
    selected_index: usize,
    candidates: &'a [Candidate],
    dims: &'a GridDimensions,
    regions: &'b mut Vec<HitRegion>,
}

/// 渲染网格内所有候选词项
fn render_grid_items(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    ctx: GridRenderContext<'_, '_>,
) {
    let s = ctx.dims.s;
    let font_size = renderer.font_size * s;
    let col_gap = 8.0 * s;

    let mut col_starts = [0.0; GRID_COLS];
    let mut acc_x = ctx.dims.padding_h;
    for (c, start) in col_starts.iter_mut().enumerate() {
        *start = acc_x;
        acc_x += ctx.dims.col_widths[c] + col_gap;
    }

    for (idx, cand) in ctx.candidates.iter().take(MAX_GRID_CAPACITY).enumerate() {
        let row = idx / GRID_COLS;
        let col = idx % GRID_COLS;
        let item_x = col_starts[col];
        let item_y = ctx.dims.padding_v + row as f32 * ctx.dims.row_height;
        let item_w = ctx.dims.col_widths[col];
        let item_h = ctx.dims.row_height;

        let is_selected = idx == ctx.selected_index;
        if is_selected {
            draw_rounded_rect(
                pixmap,
                RoundedRectParams {
                    x: item_x,
                    y: item_y + 2.0 * s,
                    w: item_w,
                    h: item_h - 4.0 * s,
                    r: 4.0 * s,
                },
                renderer.theme.highlight_badge,
            );
        }

        let baseline_y = item_y + (item_h + font_size * 0.72) / 2.0;
        let mut cur_x = item_x + 4.0 * s;

        if row == ctx.dims.selected_row {
            let num_str = renderer.index_label(col);
            if !num_str.is_empty() {
                let num_color = if is_selected {
                    renderer.theme.highlight_text
                } else {
                    renderer.theme.index_text
                };
                let num_w = renderer.measure_text(&num_str, font_size * 0.85);
                renderer.draw_text(
                    pixmap,
                    DrawTextOptions {
                        text: &num_str,
                        x: cur_x,
                        y: baseline_y,
                        size: font_size * 0.85,
                        color: num_color,
                    },
                );
                cur_x += num_w + 4.0 * s;
            }
        }

        let text_color = if is_selected {
            renderer.theme.highlight_text
        } else {
            renderer.theme.candidate_text
        };
        let display_text = renderer.display_text(cand);
        renderer.draw_text(
            pixmap,
            DrawTextOptions {
                text: &display_text,
                x: cur_x,
                y: baseline_y,
                size: font_size,
                color: text_color,
            },
        );

        ctx.regions.push(HitRegion::new(
            item_x,
            item_y,
            item_w,
            item_h,
            CandidateHitTarget::Select(idx),
        ));
    }
}

/// 渲染右侧折叠箭头、动态指示滚动条及上下翻页命中区域
fn render_grid_controls(
    renderer: &CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    dims: &GridDimensions,
    regions: &mut Vec<HitRegion>,
) {
    let s = dims.s;
    let ctrl_center_x = dims.total_width - 14.0 * s;
    let ctrl_top_y = dims.padding_v + 12.0 * s;

    // 绘制向上折叠箭头
    draw_collapse_arrow(
        pixmap,
        ctrl_center_x,
        ctrl_top_y,
        10.0 * s,
        renderer.theme.index_text,
    );
    // 记录折叠按钮点击区域
    regions.push(HitRegion::new(
        ctrl_center_x - 10.0 * s,
        ctrl_top_y - 10.0 * s,
        20.0 * s,
        20.0 * s,
        CandidateHitTarget::ToggleExpand,
    ));

    // 绘制右侧细长动态滚动条滑块
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
    draw_scrollbar(
        pixmap,
        ScrollbarParams {
            x: scroll_x,
            y: thumb_y,
            width: 3.0 * s,
            height: thumb_h,
            radius: 1.5 * s,
        },
        renderer.theme.border,
    );

    // 注册右侧上下滚动区域为上一页与下一页点击目标
    let half_scroll_h = scroll_h * 0.5;
    regions.push(HitRegion::new(
        ctrl_center_x - 10.0 * s,
        scroll_y,
        20.0 * s,
        half_scroll_h,
        CandidateHitTarget::PrevPage,
    ));
    regions.push(HitRegion::new(
        ctrl_center_x - 10.0 * s,
        scroll_y + half_scroll_h,
        20.0 * s,
        half_scroll_h,
        CandidateHitTarget::NextPage,
    ));
}
