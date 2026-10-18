//! 单行胶囊候选栏渲染与布局模块
//!
//! 负责单行胶囊候选框的背景、模式标识徽标、预编辑拼音文本、候选词项及下拉小箭头的渲染与交互区域记录。

use super::draw::{
    draw_calendar_icon, draw_card_background_at, draw_dropdown_arrow, draw_rounded_rect,
    CardBackgroundParams, RoundedRectParams,
};
use super::layout::{CandidateHitTarget, HitRegion, ItemMeasure};
use super::{CandidateBoxRenderer, DrawTextOptions};
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

/// 渲染处于语音听写状态下的动态声波胶囊栏
pub fn render_voice_capsule_bar(
    renderer: &mut CandidateBoxRenderer,
    state: &CandidateBoxState,
    hit_regions: &mut Vec<HitRegion>,
) -> Option<Pixmap> {
    let s = renderer.scale.max(0.5);
    let bar_height: f32 = 38.0 * s;
    let padding_h: f32 = renderer.theme.padding.max(6.0) * s;
    let corner_radius: f32 = renderer.theme.corner_radius.max(2.0) * s;
    let font_size = renderer.font_size * s;

    let voice = state.voice.as_ref()?;

    // 提取当前语音流式文本
    let voice_text = if let Some(ref dt) = voice.display_text {
        if !dt.is_empty() {
            dt.as_str()
        } else if !voice.partial_text.is_empty() {
            voice.partial_text.as_str()
        } else {
            "正在聆听..."
        }
    } else if !voice.partial_text.is_empty() {
        voice.partial_text.as_str()
    } else {
        "正在聆听..."
    };

    // 1. 测量波形区域：解析 waveform 字符高度比例
    let ratios: Vec<f32> = if voice.waveform.is_empty() {
        vec![0.3, 0.6, 0.8, 0.5, 0.3]
    } else {
        voice
            .waveform
            .chars()
            .map(|ch| match ch {
                ' ' => 0.15,
                '▂' => 0.25,
                '▃' => 0.40,
                '▄' => 0.55,
                '▅' => 0.70,
                '▆' => 0.82,
                '▇' => 0.92,
                '█' => 1.00,
                _ => 0.35,
            })
            .collect()
    };
    let bar_count = ratios.len().clamp(3, 10);
    let wave_bar_w = 3.0 * s;
    let wave_gap = 2.5 * s;
    let wave_total_w =
        bar_count as f32 * wave_bar_w + (bar_count.saturating_sub(1) as f32) * wave_gap;

    // 2. 测量文本宽度
    let text_w = renderer.measure_text(voice_text, font_size);
    let total_bar_width = (padding_h * 2.0 + wave_total_w + 10.0 * s + text_w).max(140.0 * s);

    let canvas_w = total_bar_width.ceil() as u32;
    let canvas_h = bar_height.ceil() as u32;
    if canvas_w == 0 || canvas_h == 0 {
        return None;
    }

    let mut pixmap = Pixmap::new(canvas_w, canvas_h)?;

    // 3. 绘制胶囊外层圆角卡片背景
    draw_card_background_at(
        &mut pixmap,
        &renderer.theme,
        CardBackgroundParams {
            x: 0.0,
            y: 0.0,
            width: total_bar_width,
            height: bar_height,
            radius: corner_radius,
            stroke_width: 1.0 * s,
        },
    );

    // 4. 绘制前置动态振幅波形柱（纯净声波直接置前，杜绝冗余模式徽标）
    let mut current_x = padding_h + 2.0 * s;
    let max_wave_h = 20.0 * s;
    let min_wave_h = 4.0 * s;
    let wave_center_y = bar_height / 2.0;

    for &ratio in ratios.iter().take(bar_count) {
        let bar_h = (min_wave_h + (max_wave_h - min_wave_h) * ratio).min(max_wave_h);
        let bar_y = wave_center_y - bar_h / 2.0;

        draw_rounded_rect(
            &mut pixmap,
            RoundedRectParams {
                x: current_x,
                y: bar_y,
                w: wave_bar_w,
                h: bar_h,
                r: 1.5 * s,
            },
            renderer.theme.highlight_badge,
        );
        current_x += wave_bar_w + wave_gap;
    }
    current_x += 10.0 * s;

    // 7. 绘制流式文字
    let baseline_y = (bar_height + font_size * 0.72) / 2.0;
    renderer.draw_text(
        &mut pixmap,
        DrawTextOptions {
            text: voice_text,
            x: current_x,
            y: baseline_y,
            size: font_size,
            color: renderer.theme.candidate_text,
        },
    );

    // 8. 记录命中区域
    hit_regions.push(HitRegion {
        x: 0.0,
        y: 0.0,
        width: total_bar_width,
        height: bar_height,
        target: CandidateHitTarget::Select(0),
    });

    Some(pixmap)
}
