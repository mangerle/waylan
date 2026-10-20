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
use crate::theme::{Color, UiTheme};
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

/// 单个网格候选项的绘制参数
struct GridItemDrawParams<'a> {
    cand: &'a Candidate,
    col: usize,
    item_x: f32,
    item_y: f32,
    item_w: f32,
    item_h: f32,
    baseline_y: f32,
    is_selected: bool,
    is_selected_row: bool,
    font_size: f32,
    scale: f32,
}

/// 网格项序号前缀绘制参数
struct GridIndexDrawParams<'a> {
    num_str: &'a str,
    x: f32,
    baseline_y: f32,
    font_size: f32,
    color: Color,
    scale: f32,
}

/// 网格折叠按钮几何参数
struct GridCollapseButtonParams {
    center_x: f32,
    top_y: f32,
    scale: f32,
}

/// 网格右侧滚动条几何参数
struct GridScrollbarParams {
    scroll_x: f32,
    scroll_y: f32,
    scroll_h: f32,
    thumb_y: f32,
    thumb_h: f32,
    scale: f32,
}

/// 计算网格各列宽度与总宽高
fn compute_grid_dimensions(
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

    let col_starts = compute_grid_col_starts(ctx.dims.padding_h, &ctx.dims.col_widths, col_gap);

    for (idx, cand) in ctx.candidates.iter().take(MAX_GRID_CAPACITY).enumerate() {
        let row = idx / GRID_COLS;
        let col = idx % GRID_COLS;
        let item_x = col_starts[col];
        let item_y = ctx.dims.padding_v + row as f32 * ctx.dims.row_height;
        let item_w = ctx.dims.col_widths[col];
        let item_h = ctx.dims.row_height;

        let is_selected = idx == ctx.selected_index;
        let baseline_y = item_y + (item_h + font_size * 0.72) / 2.0;
        draw_grid_item(
            renderer,
            pixmap,
            &GridItemDrawParams {
                cand,
                col,
                item_x,
                item_y,
                item_w,
                item_h,
                baseline_y,
                is_selected,
                is_selected_row: row == ctx.dims.selected_row,
                font_size,
                scale: s,
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

/// 计算网格各列的起始横坐标
fn compute_grid_col_starts(
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

/// 绘制单个网格候选词项（高亮底色、序号前缀与候选文本）
fn draw_grid_item(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: &GridItemDrawParams<'_>,
) {
    let s = params.scale;

    if params.is_selected {
        draw_grid_item_highlight(pixmap, &renderer.theme, params);
    }

    let mut cur_x = params.item_x + 4.0 * s;

    if params.is_selected_row {
        cur_x = draw_grid_selected_row_index(renderer, pixmap, params, cur_x);
    }

    let text_color = resolve_grid_text_color(
        &renderer.theme,
        params.is_selected,
        renderer.theme.candidate_text,
    );
    let display_text = renderer.display_text(params.cand);
    renderer.draw_text(
        pixmap,
        DrawTextOptions {
            text: &display_text,
            x: cur_x,
            y: params.baseline_y,
            size: params.font_size,
            color: text_color,
        },
    );
}

/// 绘制网格当前选中行内的序号前缀，并返回候选文本的起始横坐标
fn draw_grid_selected_row_index(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: &GridItemDrawParams<'_>,
    cur_x: f32,
) -> f32 {
    let num_str = renderer.index_label(params.col);
    let num_color = resolve_grid_text_color(
        &renderer.theme,
        params.is_selected,
        renderer.theme.index_text,
    );
    draw_grid_index(
        renderer,
        pixmap,
        &GridIndexDrawParams {
            num_str: &num_str,
            x: cur_x,
            baseline_y: params.baseline_y,
            font_size: params.font_size,
            color: num_color,
            scale: params.scale,
        },
    )
}

/// 绘制网格选中项的高亮圆角底色
fn draw_grid_item_highlight(pixmap: &mut Pixmap, theme: &UiTheme, params: &GridItemDrawParams<'_>) {
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: params.item_x,
            y: params.item_y + 2.0 * params.scale,
            w: params.item_w,
            h: params.item_h - 4.0 * params.scale,
            r: 4.0 * params.scale,
        },
        theme.highlight_badge,
    );
}

/// 绘制网格项序号前缀并返回候选文本的起始横坐标
fn draw_grid_index(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: &GridIndexDrawParams<'_>,
) -> f32 {
    if params.num_str.is_empty() {
        return params.x;
    }
    let num_w = renderer.measure_text(params.num_str, params.font_size * 0.85);
    renderer.draw_text(
        pixmap,
        DrawTextOptions {
            text: params.num_str,
            x: params.x,
            y: params.baseline_y,
            size: params.font_size * 0.85,
            color: params.color,
        },
    );
    params.x + num_w + 4.0 * params.scale
}

/// 依据选中态解析网格文本的前景色（选中高亮，否则使用常规配色）
fn resolve_grid_text_color(theme: &UiTheme, is_selected: bool, normal_color: Color) -> Color {
    if is_selected {
        theme.highlight_text
    } else {
        normal_color
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

    // 绘制向上折叠箭头并记录折叠按钮点击区域
    draw_grid_collapse_button(
        pixmap,
        &renderer.theme,
        &GridCollapseButtonParams {
            center_x: ctrl_center_x,
            top_y: ctrl_top_y,
            scale: s,
        },
        regions,
    );

    // 绘制右侧细长动态滚动条滑块
    let scrollbar = compute_grid_scrollbar(dims, ctrl_top_y, s);
    draw_grid_scrollbar(pixmap, &renderer.theme, &scrollbar);

    // 注册右侧上下滚动区域为上一页与下一页点击目标
    register_grid_scroll_regions(regions, &scrollbar, ctrl_center_x);
}

/// 绘制向上折叠箭头并登记折叠按钮命中区域
fn draw_grid_collapse_button(
    pixmap: &mut Pixmap,
    theme: &UiTheme,
    params: &GridCollapseButtonParams,
    regions: &mut Vec<HitRegion>,
) {
    draw_collapse_arrow(
        pixmap,
        params.center_x,
        params.top_y,
        10.0 * params.scale,
        theme.index_text,
    );
    regions.push(HitRegion::new(
        params.center_x - 10.0 * params.scale,
        params.top_y - 10.0 * params.scale,
        20.0 * params.scale,
        20.0 * params.scale,
        CandidateHitTarget::ToggleExpand,
    ));
}

/// 计算网格右侧滚动条滑块的几何参数
fn compute_grid_scrollbar(dims: &GridDimensions, ctrl_top_y: f32, s: f32) -> GridScrollbarParams {
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

/// 绘制右侧细长动态滚动条滑块
fn draw_grid_scrollbar(pixmap: &mut Pixmap, theme: &UiTheme, params: &GridScrollbarParams) {
    draw_scrollbar(
        pixmap,
        ScrollbarParams {
            x: params.scroll_x,
            y: params.thumb_y,
            width: 3.0 * params.scale,
            height: params.thumb_h,
            radius: 1.5 * params.scale,
        },
        theme.border,
    );
}

/// 注册右侧上下滚动区域为上一页与下一页点击目标
fn register_grid_scroll_regions(
    regions: &mut Vec<HitRegion>,
    params: &GridScrollbarParams,
    ctrl_center_x: f32,
) {
    let half_scroll_h = params.scroll_h * 0.5;
    regions.push(HitRegion::new(
        ctrl_center_x - 10.0 * params.scale,
        params.scroll_y,
        20.0 * params.scale,
        half_scroll_h,
        CandidateHitTarget::PrevPage,
    ));
    regions.push(HitRegion::new(
        ctrl_center_x - 10.0 * params.scale,
        params.scroll_y + half_scroll_h,
        20.0 * params.scale,
        half_scroll_h,
        CandidateHitTarget::NextPage,
    ));
}
