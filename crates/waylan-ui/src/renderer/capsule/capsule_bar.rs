//! 单行胶囊候选栏渲染与布局模块
//!
//! 负责单行胶囊候选框的背景、模式标识徽标、预编辑拼音文本、候选词项及下拉小箭头的渲染与交互区域记录。

use super::super::draw::{
    draw_calendar_icon, draw_card_background_at, draw_dropdown_arrow, draw_rounded_rect,
    CardBackgroundParams, RoundedRectParams,
};
use super::super::layout::{CandidateHitTarget, HitRegion, ItemMeasure};
use super::super::{CandidateBoxRenderer, DrawTextOptions};
use tiny_skia::Pixmap;
use waylan_core::{Candidate, CandidateBoxState};

/// 单行胶囊候选栏渲染布局参数
pub struct CapsuleBarParams<'a> {
    pub measured_items: &'a [ItemMeasure<'a>],
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
    draw_card_background_at(
        pixmap,
        &renderer.theme,
        CardBackgroundParams {
            x: 0.0,
            y: 0.0,
            width: params.total_bar_width,
            height: params.bar_height,
            radius: params.corner_radius,
            stroke_width: 1.0 * s,
        },
    );

    let baseline_y = (params.bar_height + params.font_size * 0.72) / 2.0;
    render_capsule_prefix(
        renderer,
        pixmap,
        state,
        CapsulePrefixParams {
            padding_h: params.padding_h,
            bar_height: params.bar_height,
            baseline_y,
            font_size: params.font_size,
            scale: s,
        },
    );

    let mut regions = Vec::with_capacity(params.measured_items.len() + 1);

    for (idx, item) in params.measured_items.iter().enumerate() {
        let cand = &state.candidates[idx];
        let is_selected = idx == state.selected_index;
        render_candidate_item(
            renderer,
            pixmap,
            CandidateItemParams {
                cand,
                item,
                is_selected,
                bar_height: params.bar_height,
                baseline_y,
                font_size: params.font_size,
                scale: s,
            },
        );

        regions.push(HitRegion::new(
            item.start_x,
            0.0,
            item.total_item_width,
            params.bar_height,
            CandidateHitTarget::Select(idx),
        ));
    }

    let action_area_width = 24.0 * s;
    let arrow_x = params.total_bar_width - action_area_width;
    let arrow_center_y = params.bar_height / 2.0;
    draw_dropdown_arrow(
        pixmap,
        &renderer.theme,
        arrow_x + 6.0 * s,
        arrow_center_y,
        s,
    );

    regions.push(HitRegion::new(
        arrow_x,
        0.0,
        action_area_width,
        params.bar_height,
        CandidateHitTarget::ToggleExpand,
    ));

    regions
}

/// 胶囊前缀绘制参数
struct CapsulePrefixParams {
    padding_h: f32,
    bar_height: f32,
    baseline_y: f32,
    font_size: f32,
    scale: f32,
}

/// 绘制前置模式标识徽标（如“中”、“英”、“V”）与预编辑拼音文本
fn render_capsule_prefix(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    state: &CandidateBoxState,
    params: CapsulePrefixParams,
) {
    let s = params.scale;
    let mut prefix_draw_x = params.padding_h + 2.0 * s;

    if !state.mode_badge.is_empty() {
        let badge_font_size = params.font_size * 0.8;
        let text_w = renderer.measure_text(&state.mode_badge, badge_font_size);
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
                text: &state.mode_badge,
                x: prefix_draw_x + 5.0 * s,
                y: (params.bar_height + badge_font_size * 0.72) / 2.0,
                size: badge_font_size,
                color: renderer.theme.highlight_text,
            },
        );
        prefix_draw_x += badge_w + 6.0 * s;
    }

    if !state.preedit.is_empty() {
        let preedit_font_size = params.font_size * 0.9;
        renderer.draw_text(
            pixmap,
            DrawTextOptions {
                text: &state.preedit,
                x: prefix_draw_x,
                y: params.baseline_y,
                size: preedit_font_size,
                color: renderer.theme.preedit_text,
            },
        );
    }
}

/// 单个候选条目绘制参数
struct CandidateItemParams<'a> {
    cand: &'a Candidate,
    item: &'a ItemMeasure<'a>,
    is_selected: bool,
    bar_height: f32,
    baseline_y: f32,
    font_size: f32,
    scale: f32,
}

/// 绘制单个候选词条目（支持高亮背景胶囊与展开日历图标）
fn render_candidate_item(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: CandidateItemParams,
) {
    let s = params.scale;
    let item = params.item;
    let cand = params.cand;

    if params.is_selected {
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
            renderer.theme.highlight_badge,
        );

        let text_x = item.start_x + 8.0 * s;
        renderer.draw_text(
            pixmap,
            DrawTextOptions {
                text: item.index_str,
                x: text_x,
                y: params.baseline_y,
                size: params.font_size * 0.85,
                color: renderer.theme.highlight_text,
            },
        );

        let mut content_x = text_x + item.index_width + 3.0 * s;
        if item.is_expandable {
            draw_calendar_icon(
                pixmap,
                &renderer.theme,
                content_x,
                badge_y + 8.5 * s,
                true,
                s,
            );
            content_x += item.icon_width;
        }

        renderer.draw_text(
            pixmap,
            DrawTextOptions {
                text: &cand.text,
                x: content_x,
                y: params.baseline_y,
                size: params.font_size,
                color: renderer.theme.highlight_text,
            },
        );
    } else {
        renderer.draw_text(
            pixmap,
            DrawTextOptions {
                text: item.index_str,
                x: item.start_x,
                y: params.baseline_y,
                size: params.font_size * 0.85,
                color: renderer.theme.index_text,
            },
        );

        let mut content_x = item.start_x + item.index_width + 4.0 * s;
        if item.is_expandable {
            let icon_y = (params.bar_height - 11.0 * s) / 2.0;
            draw_calendar_icon(pixmap, &renderer.theme, content_x, icon_y, false, s);
            content_x += item.icon_width;
        }

        renderer.draw_text(
            pixmap,
            DrawTextOptions {
                text: &cand.text,
                x: content_x,
                y: params.baseline_y,
                size: params.font_size,
                color: renderer.theme.candidate_text,
            },
        );
    }
}
