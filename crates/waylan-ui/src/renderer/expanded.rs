//! 二级展开浮层卡片几何布局与渲染绘制模块
//!
//! 负责二级展开多候选词面板的背景、高亮项、序号及文字渲染。

use super::draw::{
    draw_card_background_at, draw_rounded_rect, CardBackgroundParams, RoundedRectParams,
};
use super::layout::{CandidateHitTarget, HitRegion};
use super::{CandidateBoxRenderer, DrawTextOptions};
use crate::theme::UiTheme;
use tiny_skia::Pixmap;
use waylan_core::CandidateBoxState;

/// 二级浮层卡片几何布局与渲染参数
pub struct ExpandedCardRenderParams<'a> {
    pub state: &'a CandidateBoxState,
    pub sub_candidates: &'a [String],
    pub popup_x: f32,
    pub popup_top_y: f32,
    pub popup_w: f32,
    pub popup_h: f32,
    pub row_height: f32,
    pub corner_radius: f32,
    pub font_size: f32,
    pub scale: f32,
}

/// 二级浮层子候选条目的绘制参数
struct ExpandedSubItemParams<'a> {
    sub_str: &'a str,
    sub_num_str: &'a str,
    is_selected: bool,
    popup_x: f32,
    item_x: f32,
    row_y: f32,
    item_w: f32,
    row_height: f32,
    font_size: f32,
    scale: f32,
}

/// 绘制二级展开浮层卡片背景与子候选条目，并注册子项鼠标点击区域
pub fn render_expanded_popup_card(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    regions: &mut Vec<HitRegion>,
    params: ExpandedCardRenderParams<'_>,
) {
    let s = params.scale;
    draw_expanded_card_background(pixmap, &renderer.theme, &params);

    let item_x = params.popup_x + 6.0 * s;
    let item_w = (params.popup_w - 12.0 * s).max(1.0);

    for (s_idx, sub_str) in params.sub_candidates.iter().enumerate() {
        let row_y = params.popup_top_y + 6.0 * s + s_idx as f32 * params.row_height;
        let is_sub_selected = s_idx == params.state.expanded_selected_index();
        let sub_num_str = renderer.index_label(s_idx);
        let sub_params = ExpandedSubItemParams {
            sub_str,
            sub_num_str: &sub_num_str,
            is_selected: is_sub_selected,
            popup_x: params.popup_x,
            item_x,
            row_y,
            item_w,
            row_height: params.row_height,
            font_size: params.font_size,
            scale: s,
        };

        if is_sub_selected {
            draw_expanded_sub_item_highlight(pixmap, &renderer.theme, &sub_params);
        }

        regions.push(HitRegion::new(
            item_x,
            row_y,
            item_w,
            params.row_height,
            CandidateHitTarget::Select(s_idx),
        ));

        draw_expanded_sub_item_text(renderer, pixmap, &sub_params);
    }
}

/// 绘制二级展开浮层卡片背景
fn draw_expanded_card_background(
    pixmap: &mut Pixmap,
    theme: &UiTheme,
    params: &ExpandedCardRenderParams<'_>,
) {
    draw_card_background_at(
        pixmap,
        theme,
        CardBackgroundParams {
            x: params.popup_x,
            y: params.popup_top_y,
            width: params.popup_w,
            height: params.popup_h,
            radius: params.corner_radius,
            stroke_width: 1.0 * params.scale,
        },
    );
}

/// 绘制二级浮层子项的选中高亮底色
fn draw_expanded_sub_item_highlight(
    pixmap: &mut Pixmap,
    theme: &UiTheme,
    params: &ExpandedSubItemParams<'_>,
) {
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: params.item_x,
            y: params.row_y,
            w: params.item_w,
            h: params.row_height,
            r: 6.0 * params.scale,
        },
        theme.highlight_badge,
    );
}

/// 绘制二级浮层子项的序号前缀与候选文本
fn draw_expanded_sub_item_text(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: &ExpandedSubItemParams<'_>,
) {
    let s = params.scale;
    let sub_num_x = params.popup_x + 14.0 * s;
    let sub_baseline_y = params.row_y + (params.row_height + params.font_size * 0.72) / 2.0;
    let num_color = if params.is_selected {
        renderer.theme.highlight_text
    } else {
        renderer.theme.index_text
    };
    if !params.sub_num_str.is_empty() {
        renderer.draw_text(
            pixmap,
            DrawTextOptions {
                text: params.sub_num_str,
                x: sub_num_x,
                y: sub_baseline_y,
                size: params.font_size * 0.85,
                color: num_color,
            },
        );
    }

    let sub_text_x = sub_num_x + 14.0 * s;
    let text_color = if params.is_selected {
        renderer.theme.highlight_text
    } else {
        renderer.theme.candidate_text
    };
    renderer.draw_text(
        pixmap,
        DrawTextOptions {
            text: params.sub_str,
            x: sub_text_x,
            y: sub_baseline_y,
            size: params.font_size,
            color: text_color,
        },
    );
}
