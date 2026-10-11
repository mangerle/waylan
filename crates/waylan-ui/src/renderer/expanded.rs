//! 二级展开浮层卡片几何布局与渲染绘制模块
//!
//! 负责微信键盘风格二级展开多候选词面板的背景、高亮项、序号及文字渲染。

use super::draw::{
    draw_card_background_at, draw_rounded_rect, CardBackgroundParams, RoundedRectParams,
};
use super::layout::{CandidateHitTarget, HitRegion, PRECACHED_INDEX_STRINGS};
use super::CandidateBoxRenderer;
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

/// 绘制微信键盘风格二级展开浮层卡片背景与子候选条目，并注册子项鼠标点击区域
pub fn render_expanded_popup_card(
    renderer: &CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    regions: &mut Vec<HitRegion>,
    params: ExpandedCardRenderParams<'_>,
) {
    let s = params.scale;
    draw_card_background_at(
        pixmap,
        &renderer.theme,
        CardBackgroundParams {
            x: params.popup_x,
            y: params.popup_top_y,
            width: params.popup_w,
            height: params.popup_h,
            radius: params.corner_radius,
            stroke_width: 1.0 * s,
        },
    );

    let item_x = params.popup_x + 6.0 * s;
    let item_w = (params.popup_w - 12.0 * s).max(1.0);

    for (s_idx, sub_str) in params.sub_candidates.iter().enumerate() {
        let row_y = params.popup_top_y + 6.0 * s + s_idx as f32 * params.row_height;
        let is_sub_selected = s_idx == params.state.expanded_selected_index();
        let sub_num_str = if s_idx < PRECACHED_INDEX_STRINGS.len() {
            PRECACHED_INDEX_STRINGS[s_idx]
        } else {
            "•"
        };

        if is_sub_selected {
            draw_rounded_rect(
                pixmap,
                RoundedRectParams {
                    x: item_x,
                    y: row_y,
                    w: item_w,
                    h: params.row_height,
                    r: 6.0 * s,
                },
                renderer.theme.highlight_badge,
            );
        }

        regions.push(HitRegion::new(
            item_x,
            row_y,
            item_w,
            params.row_height,
            CandidateHitTarget::Select(s_idx),
        ));

        let sub_num_x = params.popup_x + 14.0 * s;
        let sub_baseline_y = row_y + (params.row_height + params.font_size * 0.72) / 2.0;
        let num_color = if is_sub_selected {
            renderer.theme.highlight_text
        } else {
            renderer.theme.index_text
        };
        renderer.draw_text(
            pixmap,
            sub_num_str,
            sub_num_x,
            sub_baseline_y,
            params.font_size * 0.85,
            num_color,
        );

        let sub_text_x = sub_num_x + 14.0 * s;
        let text_color = if is_sub_selected {
            renderer.theme.highlight_text
        } else {
            renderer.theme.candidate_text
        };
        renderer.draw_text(
            pixmap,
            sub_str,
            sub_text_x,
            sub_baseline_y,
            params.font_size,
            text_color,
        );
    }
}
