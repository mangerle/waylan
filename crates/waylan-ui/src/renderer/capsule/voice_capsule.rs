//! 语音声波胶囊栏渲染与振幅可视化模块
//!
//! 负责在语音听写与实时转录期间渲染声波动态振幅条、提示文本与点击响应区域。

use super::super::draw::{
    draw_card_background_at, draw_rounded_rect, CardBackgroundParams, RoundedRectParams,
};
use super::super::layout::{CandidateHitTarget, HitRegion};
use super::super::{CandidateBoxRenderer, DrawTextOptions};
use tiny_skia::Pixmap;
use waylan_core::{VoiceBoxState, VoiceVisualEffect};

use crate::theme::{Color, UiTheme};

/// 语音声波绘制参数对象
struct WaveformBarsParams<'a> {
    start_x: f32,
    ratios: &'a [f32],
    scale: f32,
    bar_height: f32,
    badge_color: Color,
}

/// 语音胶囊栏基础度量参数
struct VoiceCapsuleMetrics {
    scale: f32,
    bar_height: f32,
    padding_h: f32,
    corner_radius: f32,
    font_size: f32,
}

/// 语音胶囊前置可视化绘制参数
struct VoiceLeadingVisualParams<'a> {
    effect: VoiceVisualEffect,
    ratios: &'a [f32],
    scale: f32,
    bar_height: f32,
    start_x: f32,
    badge_color: Color,
}

/// 语音胶囊内容区（前置可视化 + 提示文本）绘制参数
struct VoiceContentParams<'a> {
    effect: VoiceVisualEffect,
    ratios: &'a [f32],
    voice_text: &'a str,
    scale: f32,
    bar_height: f32,
    padding_h: f32,
    font_size: f32,
    badge_color: Color,
    text_color: Color,
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

/// 依据渲染器样式解析语音胶囊栏的基础度量参数
fn resolve_voice_capsule_metrics(renderer: &CandidateBoxRenderer) -> VoiceCapsuleMetrics {
    let s = renderer.scale.max(0.5);
    VoiceCapsuleMetrics {
        scale: s,
        bar_height: renderer.style.bar_height * s,
        padding_h: renderer.style.padding_h.max(6.0) * s,
        corner_radius: renderer.style.corner_radius.max(2.0) * s,
        font_size: renderer.font_size * s,
    }
}

/// 计算声波柱序列所占用的总宽度
fn compute_voice_wave_total_width(ratio_count: usize, s: f32) -> f32 {
    let bar_count = ratio_count.clamp(3, 10);
    let wave_bar_w = 3.0 * s;
    let wave_gap = 2.5 * s;
    bar_count as f32 * wave_bar_w + (bar_count.saturating_sub(1) as f32) * wave_gap
}

/// 依据视觉动效类型解析前置区域的占位宽度
fn resolve_voice_leading_width(effect: VoiceVisualEffect, wave_total_w: f32, s: f32) -> f32 {
    match effect {
        VoiceVisualEffect::DynamicWaveform => wave_total_w + 10.0 * s,
        VoiceVisualEffect::BreathingPulse => 18.0 * s + 4.0 * s,
        VoiceVisualEffect::ClassicText => 0.0,
    }
}

/// 创建语音胶囊栏画布（画布尺寸非法时返回 None）
fn create_voice_capsule_pixmap(total_bar_width: f32, bar_height: f32) -> Option<Pixmap> {
    let canvas_w = total_bar_width.ceil() as u32;
    let canvas_h = bar_height.ceil() as u32;
    if canvas_w == 0 || canvas_h == 0 {
        return None;
    }
    Pixmap::new(canvas_w, canvas_h)
}

/// 绘制语音胶囊栏外层圆角卡片背景
fn draw_voice_capsule_background(
    pixmap: &mut Pixmap,
    theme: &UiTheme,
    metrics: &VoiceCapsuleMetrics,
    total_bar_width: f32,
) {
    draw_card_background_at(
        pixmap,
        theme,
        CardBackgroundParams {
            x: 0.0,
            y: 0.0,
            width: total_bar_width,
            height: metrics.bar_height,
            radius: metrics.corner_radius,
            stroke_width: 1.0 * metrics.scale,
        },
    );
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

/// 呼吸光环圆点绘制参数
///
/// 承载呼吸光效的起始横坐标、胶囊高度、缩放系数与配色，供 `draw_breathing_pulse`
/// 以单次引用传递，避免平铺多个标量入参。
#[derive(Debug, Clone, Copy)]
struct BreathingPulseParams {
    /// 光环绘制区域的起始横坐标
    start_x: f32,
    /// 胶囊条高度（用于计算光环纵向中心）
    bar_height: f32,
    /// 缩放系数
    scale: f32,
    /// 光环主体颜色
    color: Color,
}

/// 绘制呼吸光环圆点并返回结束 x 坐标
fn draw_breathing_pulse(pixmap: &mut Pixmap, params: &BreathingPulseParams) -> f32 {
    let start_x = params.start_x;
    let bar_height = params.bar_height;
    let scale = params.scale;
    let color = params.color;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let phase = (now_ms % 1600) as f32 / 1600.0 * std::f32::consts::TAU;
    let breathe = phase.sin() * 0.5 + 0.5;
    let outer_r = (7.0 + 3.0 * breathe) * scale;
    let center_x = start_x + 8.0 * scale;
    let center_y = bar_height / 2.0;

    let mut halo = color;
    halo.a = 0.22 + 0.12 * breathe;
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: center_x - outer_r,
            y: center_y - outer_r,
            w: outer_r * 2.0,
            h: outer_r * 2.0,
            r: outer_r,
        },
        halo,
    );

    let inner_r = 4.5 * scale;
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: center_x - inner_r,
            y: center_y - inner_r,
            w: inner_r * 2.0,
            h: inner_r * 2.0,
            r: inner_r,
        },
        color,
    );
    start_x + 18.0 * scale
}

/// 绘制语音胶囊前置动态可视化并返回其结束横坐标
fn draw_voice_leading_visual(pixmap: &mut Pixmap, params: &VoiceLeadingVisualParams<'_>) -> f32 {
    let s = params.scale;
    match params.effect {
        VoiceVisualEffect::DynamicWaveform => {
            draw_voice_waveform_bars(
                pixmap,
                WaveformBarsParams {
                    start_x: params.start_x,
                    ratios: params.ratios,
                    scale: s,
                    bar_height: params.bar_height,
                    badge_color: params.badge_color,
                },
            ) + 10.0 * s
        }
        VoiceVisualEffect::BreathingPulse => {
            draw_breathing_pulse(
                pixmap,
                &BreathingPulseParams {
                    start_x: params.start_x,
                    bar_height: params.bar_height,
                    scale: s,
                    color: params.badge_color,
                },
            ) + 4.0 * s
        }
        VoiceVisualEffect::ClassicText => params.start_x,
    }
}

/// 绘制语音胶囊内容区（先前置可视化后提示文本，严格保持绘制顺序）
fn draw_voice_content(
    renderer: &mut CandidateBoxRenderer,
    pixmap: &mut Pixmap,
    params: &VoiceContentParams<'_>,
) {
    let s = params.scale;
    let start_x = params.padding_h + 2.0 * s;
    let current_x = draw_voice_leading_visual(
        pixmap,
        &VoiceLeadingVisualParams {
            effect: params.effect,
            ratios: params.ratios,
            scale: s,
            bar_height: params.bar_height,
            start_x,
            badge_color: params.badge_color,
        },
    );

    let baseline_y = (params.bar_height + params.font_size * 0.72) / 2.0;
    renderer.draw_text(
        pixmap,
        DrawTextOptions {
            text: params.voice_text,
            x: current_x,
            y: baseline_y,
            size: params.font_size,
            color: params.text_color,
        },
    );
}

/// 渲染处于语音听写状态下的动态声波胶囊栏
pub fn render_voice_capsule_bar(
    renderer: &mut CandidateBoxRenderer,
    voice: &VoiceBoxState,
    hit_regions: &mut Vec<HitRegion>,
) -> Option<Pixmap> {
    let metrics = resolve_voice_capsule_metrics(renderer);
    let s = metrics.scale;

    let voice_text = resolve_voice_text(voice);
    let ratios = parse_waveform_ratios(&voice.waveform);

    let wave_total_w = compute_voice_wave_total_width(ratios.len(), s);
    let leading_width = resolve_voice_leading_width(voice.visual_effect, wave_total_w, s);
    let text_w = renderer.measure_text(voice_text, metrics.font_size);
    let total_bar_width = (metrics.padding_h * 2.0 + leading_width + text_w).max(140.0 * s);

    let mut pixmap = create_voice_capsule_pixmap(total_bar_width, metrics.bar_height)?;

    draw_voice_capsule_background(&mut pixmap, &renderer.theme, &metrics, total_bar_width);

    let badge_color = renderer.theme.highlight_badge;
    let text_color = renderer.theme.candidate_text;
    draw_voice_content(
        renderer,
        &mut pixmap,
        &VoiceContentParams {
            effect: voice.visual_effect,
            ratios: &ratios,
            voice_text,
            scale: s,
            bar_height: metrics.bar_height,
            padding_h: metrics.padding_h,
            font_size: metrics.font_size,
            badge_color,
            text_color,
        },
    );

    register_voice_hit_region(hit_regions, total_bar_width, metrics.bar_height);

    Some(pixmap)
}

/// 注册语音胶囊栏整体的点击响应区域
fn register_voice_hit_region(
    hit_regions: &mut Vec<HitRegion>,
    total_bar_width: f32,
    bar_height: f32,
) {
    hit_regions.push(HitRegion {
        x: 0.0,
        y: 0.0,
        width: total_bar_width,
        height: bar_height,
        target: CandidateHitTarget::Select(0),
    });
}
