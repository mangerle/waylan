//! 单行胶囊候选栏渲染与布局模块
//!
//! 负责单行胶囊候选框的背景、模式标识徽标、预编辑拼音文本、候选词项及下拉小箭头的渲染与交互区域记录。

use super::super::draw::{
    CalendarIconParams, CardBackgroundParams, DropdownArrowParams, RoundedRectParams,
    draw_calendar_icon, draw_card_background_at, draw_dropdown_arrow, draw_rounded_rect,
};
use super::super::layout::{CandidateHitTarget, HitRegion, HitRegionParams, ItemMeasure};
use super::super::{CandidateBoxRenderer, DrawTextOptions};
use crate::theme::{Color, UiTheme};
use tiny_skia::Pixmap;
use waylan_core::{Candidate, CandidateBoxState};

/// 单行胶囊候选栏渲染布局参数
pub struct CapsuleBarParams<'a> {
    pub measured_items: &'a [ItemMeasure],
    pub total_bar_width: f32,
    pub bar_height: f32,
    pub padding_h: f32,
    pub corner_radius: f32,
    pub font_size: f32,
    pub scale: f32,
}

/// 渲染单行胶囊候选条背景、徽标、拼音、候选词项与下拉按钮
pub fn render_capsule_bar(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    state: &CandidateBoxState,
    params: CapsuleBarParams,
) -> Vec<HitRegion> {
    let s = params.scale;

    // 绘制胶囊外层卡片圆角背景
    draw_capsule_bar_background(pixmap, &renderer.theme, &params);

    let baseline_y = (params.bar_height + params.font_size * 0.72) / 2.0;
    render_capsule_prefix(
        renderer,
        pixmap,
        CapsulePrefixParams {
            padding_h: params.padding_h,
            bar_height: params.bar_height,
            baseline_y,
            font_size: params.font_size,
            scale: s,
            mode_badge: &state.mode_badge,
            preedit: &state.preedit,
        },
    );

    let mut regions = Vec::with_capacity(params.measured_items.len() + 1);
    render_capsule_candidates(
        renderer,
        pixmap,
        &CapsuleCandidateContext {
            candidates: &state.candidates,
            selected_index: state.selected_index,
            measured_items: params.measured_items,
            bar_height: params.bar_height,
            baseline_y,
            font_size: params.font_size,
            scale: s,
        },
        &mut regions,
    );
    draw_capsule_dropdown(pixmap, &renderer.theme, &params, &mut regions);

    regions
}

/// 绘制胶囊候选条外层圆角卡片背景
fn draw_capsule_bar_background(pixmap: &mut Pixmap, theme: &UiTheme, params: &CapsuleBarParams) {
    draw_card_background_at(
        pixmap,
        theme,
        CardBackgroundParams {
            x: 0.0,
            y: 0.0,
            width: params.total_bar_width,
            height: params.bar_height,
            radius: params.corner_radius,
            stroke_width: 1.0 * params.scale,
        },
    );
}

/// 胶囊候选词条批量渲染上下文，聚合候选数据、度量结果与几何参数
struct CapsuleCandidateContext<'a> {
    candidates: &'a [Candidate],
    selected_index: usize,
    measured_items: &'a [ItemMeasure],
    bar_height: f32,
    baseline_y: f32,
    font_size: f32,
    scale: f32,
}

/// 逐项渲染胶囊候选词条并同步登记对应命中区域
fn render_capsule_candidates(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    ctx: &CapsuleCandidateContext<'_>,
    regions: &mut Vec<HitRegion>,
) {
    for (idx, item) in ctx.measured_items.iter().enumerate() {
        let cand = &ctx.candidates[idx];
        let is_selected = idx == ctx.selected_index;
        render_candidate_item(
            renderer,
            pixmap,
            CandidateItemParams {
                cand,
                item,
                is_selected,
                bar_height: ctx.bar_height,
                baseline_y: ctx.baseline_y,
                font_size: ctx.font_size,
                scale: ctx.scale,
            },
        );

        regions.push(HitRegion::new(HitRegionParams {
            x: item.start_x,
            y: 0.0,
            width: item.total_item_width,
            height: ctx.bar_height,
            target: CandidateHitTarget::Select(idx),
        }));
    }
}

/// 绘制胶囊右端下拉小箭头并登记展开切换命中区域
fn draw_capsule_dropdown(
    pixmap: &mut Pixmap,
    theme: &UiTheme,
    params: &CapsuleBarParams,
    regions: &mut Vec<HitRegion>,
) {
    let action_area_width = 24.0 * params.scale;
    let arrow_x = params.total_bar_width - action_area_width;
    let arrow_center_y = params.bar_height / 2.0;
    draw_dropdown_arrow(
        pixmap,
        theme,
        &DropdownArrowParams {
            x: arrow_x + 6.0 * params.scale,
            y: arrow_center_y,
            scale: params.scale,
        },
    );

    regions.push(HitRegion::new(HitRegionParams {
        x: arrow_x,
        y: 0.0,
        width: action_area_width,
        height: params.bar_height,
        target: CandidateHitTarget::ToggleExpand,
    }));
}

/// 胶囊前缀绘制参数
struct CapsulePrefixParams<'a> {
    padding_h: f32,
    bar_height: f32,
    baseline_y: f32,
    font_size: f32,
    scale: f32,
    mode_badge: &'a str,
    preedit: &'a str,
}

/// 绘制前置模式标识徽标（如“中”、“英”、“V”）与预编辑拼音文本
fn render_capsule_prefix(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: CapsulePrefixParams<'_>,
) {
    let s = params.scale;
    let mut prefix_draw_x = params.padding_h + 2.0 * s;

    if !params.mode_badge.is_empty() {
        prefix_draw_x = draw_mode_badge(renderer, pixmap, &params, prefix_draw_x);
    }

    if !params.preedit.is_empty() {
        draw_preedit_text(renderer, pixmap, &params, prefix_draw_x);
    }
}

/// 绘制模式标识徽标并返回徽标之后的前缀绘制横坐标
fn draw_mode_badge(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: &CapsulePrefixParams<'_>,
    prefix_draw_x: f32,
) -> f32 {
    let s = params.scale;
    let badge_font_size = params.font_size * 0.8;
    let text_w = renderer.measure_text(params.mode_badge, badge_font_size);
    let badge_w = text_w + 10.0 * s;
    let badge_h = 22.0 * s;
    let badge_y = (params.bar_height - badge_h) / 2.0;

    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: prefix_draw_x,
            y: badge_y,
            w: badge_w,
            h: badge_h,
            r: 4.0 * s,
        },
        renderer.theme.highlight_badge,
    );
    renderer.draw_text(
        pixmap,
        DrawTextOptions {
            text: params.mode_badge,
            x: prefix_draw_x + 5.0 * s,
            y: (params.bar_height + badge_font_size * 0.72) / 2.0,
            size: badge_font_size,
            color: renderer.theme.highlight_text,
        },
    );
    prefix_draw_x + badge_w + 6.0 * s
}

/// 绘制预编辑拼音文本（沿用当前前缀绘制横坐标向右延展）
fn draw_preedit_text(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: &CapsulePrefixParams<'_>,
    prefix_draw_x: f32,
) {
    let preedit_font_size = params.font_size * 0.9;
    renderer.draw_text(
        pixmap,
        DrawTextOptions {
            text: params.preedit,
            x: prefix_draw_x,
            y: params.baseline_y,
            size: preedit_font_size,
            color: renderer.theme.preedit_text,
        },
    );
}

/// 单个候选条目绘制参数
struct CandidateItemParams<'a> {
    cand: &'a Candidate,
    item: &'a ItemMeasure,
    is_selected: bool,
    bar_height: f32,
    baseline_y: f32,
    font_size: f32,
    scale: f32,
}

/// 候选词序号前缀绘制参数
struct CandidateIndexDrawParams<'a> {
    index_str: &'a str,
    x: f32,
    baseline_y: f32,
    font_size: f32,
    color: Color,
}

/// 绘制候选词序号前缀（序号为空时跳过绘制，保持与原实现一致的留白效果）
fn draw_candidate_index(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: &CandidateIndexDrawParams<'_>,
) {
    if params.index_str.is_empty() {
        return;
    }
    renderer.draw_text(
        pixmap,
        DrawTextOptions {
            text: params.index_str,
            x: params.x,
            y: params.baseline_y,
            size: params.font_size * 0.85,
            color: params.color,
        },
    );
}

/// 绘制单个候选词条目（支持高亮背景胶囊与展开日历图标）
fn render_candidate_item(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: CandidateItemParams,
) {
    let s = params.scale;
    let item = params.item;
    let is_selected = params.is_selected;

    let (text_x, icon_y, index_color, text_color, index_gap) = if is_selected {
        let badge_y = draw_selected_item_highlight(pixmap, &renderer.theme, &params);
        (
            item.start_x + 8.0 * s,
            badge_y + 8.5 * s,
            renderer.theme.highlight_text,
            renderer.theme.highlight_text,
            3.0 * s,
        )
    } else {
        (
            item.start_x,
            (params.bar_height - 11.0 * s) / 2.0,
            renderer.theme.index_text,
            renderer.theme.candidate_text,
            4.0 * s,
        )
    };

    draw_candidate_index(
        renderer,
        pixmap,
        &CandidateIndexDrawParams {
            index_str: &item.index_str,
            x: text_x,
            baseline_y: params.baseline_y,
            font_size: params.font_size,
            color: index_color,
        },
    );

    let mut content_x = text_x + item.index_width + index_gap;
    if item.is_expandable {
        draw_calendar_icon(
            pixmap,
            &renderer.theme,
            &CalendarIconParams {
                x: content_x,
                y: icon_y,
                is_selected,
                scale: s,
            },
        );
        content_x += item.icon_width;
    }

    let display_text = renderer.display_text(params.cand);
    renderer.draw_text(
        pixmap,
        DrawTextOptions {
            text: &display_text,
            x: content_x,
            y: params.baseline_y,
            size: params.font_size,
            color: text_color,
        },
    );
}

/// 绘制选中态候选条目的高亮胶囊底色，并返回其纵向起始坐标
fn draw_selected_item_highlight(
    pixmap: &mut Pixmap,
    theme: &UiTheme,
    params: &CandidateItemParams<'_>,
) -> f32 {
    let s = params.scale;
    let item = params.item;
    let badge_h = 28.0 * s;
    let badge_y = (params.bar_height - badge_h) / 2.0;
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: item.start_x,
            y: badge_y,
            w: item.total_item_width,
            h: badge_h,
            r: 6.0 * s,
        },
        theme.highlight_badge,
    );
    badge_y
}
