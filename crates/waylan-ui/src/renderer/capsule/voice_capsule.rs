//! 语音声波胶囊栏渲染与振幅可视化模块
//!
//! 负责在语音听写与实时转录期间渲染声波动态振幅条、提示文本与点击响应区域。

use super::super::draw::{
    draw_card_background_at, draw_rounded_rect, CardBackgroundParams, RoundedRectParams,
};
use super::super::layout::{CandidateHitTarget, HitRegion};
use super::super::{CandidateBoxRenderer, DrawTextOptions};
use tiny_skia::Pixmap;
use waylan_core::VoiceBoxState;

use crate::theme::Color;

/// 语音声波绘制参数对象
struct WaveformBarsParams<'a> {
    start_x: f32,
    ratios: &'a [f32],
    scale: f32,
    bar_height: f32,
    badge_color: Color,
}

/// 解析语音听写待显示的文本内容
fn resolve_voice_text(voice: &VoiceBoxState) -> &str {
    if let Some(ref dt) = voice.display_text {
        if !dt.is_empty() {
            return dt.as_str();
        }
    }
    if !voice.partial_text.is_empty() {
        voice.partial_text.as_str()
    } else {
        "正在聆听..."
    }
}

/// 解析波形字符序列为归一化振幅比例列表
fn parse_waveform_ratios(waveform: &str) -> Vec<f32> {
    if waveform.is_empty() {
        return vec![0.3, 0.6, 0.8, 0.5, 0.3];
    }
    waveform
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
}

/// 绘制前置动态振幅波形柱并返回结束 x 坐标
fn draw_voice_waveform_bars(pixmap: &mut Pixmap, params: WaveformBarsParams<'_>) -> f32 {
    let bar_count = params.ratios.len().clamp(3, 10);
    let wave_bar_w = 3.0 * params.scale;
    let wave_gap = 2.5 * params.scale;
    let max_wave_h = 20.0 * params.scale;
    let min_wave_h = 4.0 * params.scale;
    let wave_center_y = params.bar_height / 2.0;

    let mut current_x = params.start_x;
    for &ratio in params.ratios.iter().take(bar_count) {
        let bar_h = (min_wave_h + (max_wave_h - min_wave_h) * ratio).min(max_wave_h);
        let bar_y = wave_center_y - bar_h / 2.0;

        draw_rounded_rect(
            pixmap,
            RoundedRectParams {
                x: current_x,
                y: bar_y,
                w: wave_bar_w,
                h: bar_h,
                r: 1.5 * params.scale,
            },
            params.badge_color,
        );
        current_x += wave_bar_w + wave_gap;
    }
    current_x
}

/// 渲染处于语音听写状态下的动态声波胶囊栏
pub fn render_voice_capsule_bar(
    renderer: &mut CandidateBoxRenderer,
    voice: &VoiceBoxState,
    hit_regions: &mut Vec<HitRegion>,
) -> Option<Pixmap> {
    let s = renderer.scale.max(0.5);
    let bar_height: f32 = 38.0 * s;
    let padding_h: f32 = renderer.theme.padding.max(6.0) * s;
    let corner_radius: f32 = renderer.theme.corner_radius.max(2.0) * s;
    let font_size = renderer.font_size * s;

    let voice_text = resolve_voice_text(voice);
    let ratios = parse_waveform_ratios(&voice.waveform);

    let bar_count = ratios.len().clamp(3, 10);
    let wave_bar_w = 3.0 * s;
    let wave_gap = 2.5 * s;
    let wave_total_w =
        bar_count as f32 * wave_bar_w + (bar_count.saturating_sub(1) as f32) * wave_gap;

    let text_w = renderer.measure_text(voice_text, font_size);
    let total_bar_width = (padding_h * 2.0 + wave_total_w + 10.0 * s + text_w).max(140.0 * s);

    let canvas_w = total_bar_width.ceil() as u32;
    let canvas_h = bar_height.ceil() as u32;
    if canvas_w == 0 || canvas_h == 0 {
        return None;
    }

    let mut pixmap = Pixmap::new(canvas_w, canvas_h)?;

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

    let start_x = padding_h + 2.0 * s;
    let current_x = draw_voice_waveform_bars(
        &mut pixmap,
        WaveformBarsParams {
            start_x,
            ratios: &ratios,
            scale: s,
            bar_height,
            badge_color: renderer.theme.highlight_badge,
        },
    ) + 10.0 * s;

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

    hit_regions.push(HitRegion {
        x: 0.0,
        y: 0.0,
        width: total_bar_width,
        height: bar_height,
        target: CandidateHitTarget::Select(0),
    });

    Some(pixmap)
}
