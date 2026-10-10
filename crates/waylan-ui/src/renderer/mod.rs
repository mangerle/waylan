pub mod draw;
pub mod font;
pub mod layout;

use crate::theme::{Color, UiTheme};
use draw::{
    draw_calendar_icon, draw_card_background_at, draw_dropdown_arrow, draw_rounded_rect,
    CardBackgroundParams, RoundedRectParams,
};
pub use font::CachedGlyph;
use font::FontManager;
use fontdue::{Font, FontSettings};
use layout::{measure_candidates_bar, LayoutParams, PRECACHED_INDEX_STRINGS};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use tiny_skia::Pixmap;
use waylan_core::CandidateBoxState;

/// 最大字形位图缓存条目数（覆盖绝大多数常用汉字与符号，占用内存不足 1MB）
const MAX_GLYPH_CACHE_SIZE: usize = 2048;

/// 候选框矢量渲染器（1:1 对标现代单行胶囊样式）
pub struct CandidateBoxRenderer {
    pub theme: UiTheme,
    font: Option<Font>,
    /// 基础候选字号大小
    pub font_size: f32,
    /// 屏幕缩放因子（HiDPI 支持，默认 1.0）
    pub scale: f32,
    /// 字形位图与度量信息缓存（避免高频字符每次击键重复矢量光栅化）
    glyph_cache: Mutex<HashMap<(char, u32), CachedGlyph>>,
}

impl CandidateBoxRenderer {
    pub fn new(theme: UiTheme) -> Self {
        let mut renderer = Self {
            theme,
            font: None,
            font_size: 15.0,
            scale: 1.0,
            glyph_cache: Mutex::new(HashMap::with_capacity(256)),
        };
        renderer.load_default_fonts();
        renderer
    }

    /// 设置 HiDPI 屏幕缩放因子
    pub fn set_scale(&mut self, scale: f32) {
        let normalized = scale.max(0.5);
        if (self.scale - normalized).abs() > 0.001 {
            self.scale = normalized;
            self.clear_glyph_cache();
        }
    }

    /// 查询当前是否已加载有效字体
    pub fn has_font(&self) -> bool {
        self.font.is_some()
    }

    /// 手动从指定文件路径加载字体
    pub fn load_font_from_path(&mut self, path: &Path) -> bool {
        std::fs::read(path)
            .map(|data| self.load_font_from_bytes(&data))
            .unwrap_or(false)
    }

    /// 从内存字节流中加载字体
    pub fn load_font_from_bytes(&mut self, bytes: &[u8]) -> bool {
        match Font::from_bytes(bytes, FontSettings::default()) {
            Ok(font) => {
                self.font = Some(font);
                self.clear_glyph_cache();
                true
            }
            Err(_) => false,
        }
    }

    /// 获取当前字形缓存条目总数（用于诊断与测试）
    pub fn glyph_cache_len(&self) -> usize {
        self.glyph_cache.lock().map(|c| c.len()).unwrap_or(0)
    }

    /// 清空字形缓存
    pub fn clear_glyph_cache(&self) {
        if let Ok(mut cache) = self.glyph_cache.lock() {
            cache.clear();
        }
    }

    /// 测量一段文字的排版宽度
    pub fn measure_text(&self, text: &str, size: f32) -> f32 {
        let Some(font) = self.font.as_ref() else {
            return text.chars().count() as f32 * (size * 0.8 + 3.0);
        };

        let size_key = (size * 100.0).round() as u32;
        let mut width = 0.0;
        for ch in text.chars() {
            if let Ok(cache) = self.glyph_cache.lock() {
                if let Some(cached) = cache.get(&(ch, size_key)) {
                    width += cached.metrics.advance_width;
                    continue;
                }
            }
            let metrics = font.metrics(ch, size);
            width += metrics.advance_width;
        }
        width
    }

    /// 获取或光栅化指定字符的字形位图
    fn get_or_rasterize_glyph(&self, ch: char, size: f32) -> Option<CachedGlyph> {
        let size_key = (size * 100.0).round() as u32;
        let font = self.font.as_ref()?;

        if let Ok(cache) = self.glyph_cache.lock() {
            if let Some(cached) = cache.get(&(ch, size_key)) {
                return Some(cached.clone());
            }
        }

        let (metrics, bitmap) = font.rasterize(ch, size);
        let cached = CachedGlyph { metrics, bitmap };
        if let Ok(mut cache) = self.glyph_cache.lock() {
            if cache.len() >= MAX_GLYPH_CACHE_SIZE {
                cache.clear();
            }
            cache.insert((ch, size_key), cached.clone());
        }
        Some(cached)
    }

    /// 加载默认系统字体
    fn load_default_fonts(&mut self) {
        self.font = FontManager::load_default_fonts();
    }

    /// 绘制文本
    fn draw_text(&self, pixmap: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, color: Color) {
        if self.font.is_none() {
            let s = self.scale.max(0.5);
            let mut current_x = x;
            let glyph_w = size * 0.8;
            let glyph_h = size;
            let placeholder_color =
                Color::from_rgba(color.r, color.g, color.b, (color.a * 0.35).clamp(0.0, 1.0));
            for _ in text.chars() {
                let rect_params = RoundedRectParams {
                    x: current_x,
                    y: y - glyph_h,
                    w: glyph_w,
                    h: glyph_h,
                    r: 2.0 * s,
                };
                draw_rounded_rect(pixmap, rect_params, placeholder_color);
                current_x += glyph_w + 3.0 * s;
            }
            return;
        }

        let mut current_x = x;
        for ch in text.chars() {
            if let Some(glyph) = self.get_or_rasterize_glyph(ch, size) {
                if glyph.metrics.width > 0 && glyph.metrics.height > 0 {
                    let glyph_x = (current_x + glyph.metrics.xmin as f32) as i32;
                    let glyph_y =
                        (y - glyph.metrics.height as f32 - glyph.metrics.ymin as f32) as i32;
                    self.blend_glyph(pixmap, &glyph, (glyph_x, glyph_y), color);
                }
                current_x += glyph.metrics.advance_width;
            }
        }
    }

    /// 混合字形位图（采用标准 Porter-Duff Source-Over）
    fn blend_glyph(&self, pixmap: &mut Pixmap, glyph: &CachedGlyph, pos: (i32, i32), color: Color) {
        let (x, y) = pos;
        let width = glyph.metrics.width;
        let height = glyph.metrics.height;
        let bitmap = &glyph.bitmap;
        let pixmap_w = pixmap.width() as i32;
        let pixmap_h = pixmap.height() as i32;
        let pixels = pixmap.pixels_mut();

        for row in 0..height {
            let py = y + row as i32;
            if py < 0 || py >= pixmap_h {
                continue;
            }

            for col in 0..width {
                let px = x + col as i32;
                if px < 0 || px >= pixmap_w {
                    continue;
                }

                let alpha = bitmap[row * width + col] as f32 / 255.0;
                if alpha <= 0.01 {
                    continue;
                }

                let pixel_idx = (py * pixmap_w + px) as usize;
                let target = &mut pixels[pixel_idx];

                let src_a = (color.a * alpha).clamp(0.0, 1.0);
                let inv_sa = 1.0 - src_a;

                let src_r = color.r * src_a * 255.0;
                let src_g = color.g * src_a * 255.0;
                let src_b = color.b * src_a * 255.0;
                let src_a_255 = src_a * 255.0;

                let out_a = (src_a_255 + target.alpha() as f32 * inv_sa)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let out_r = (src_r + target.red() as f32 * inv_sa)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let out_g = (src_g + target.green() as f32 * inv_sa)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let out_b = (src_b + target.blue() as f32 * inv_sa)
                    .round()
                    .clamp(0.0, 255.0) as u8;

                if let Some(blended) = tiny_skia::PremultipliedColorU8::from_rgba(
                    out_r.min(out_a),
                    out_g.min(out_a),
                    out_b.min(out_a),
                    out_a,
                ) {
                    *target = blended;
                }
            }
        }
    }

    /// 渲染单行胶囊候选栏及微信键盘风格二级展开浮层卡片
    pub fn render(&self, state: &CandidateBoxState) -> Option<Pixmap> {
        if state.candidates.is_empty() {
            return None;
        }

        let s = self.scale.max(0.5);
        let bar_height: f32 = 38.0 * s;
        let padding_h: f32 = 8.0 * s;
        let item_gap: f32 = 14.0 * s;
        let font_size = self.font_size * s;

        let (measured_items, total_bar_width) = measure_candidates_bar(
            LayoutParams {
                candidates: &state.candidates,
                selected_index: state.selected_index,
                s,
                padding_h,
                item_gap,
                font_size,
            },
            |text, size| self.measure_text(text, size),
        );

        let active_cand = state.candidates.get(state.selected_index);
        let has_active_sub = state.is_expanded()
            && active_cand
                .map(|c| c.is_expandable && !c.sub_candidates.is_empty())
                .unwrap_or(false);

        let sub_candidates: &[String] = if has_active_sub {
            active_cand
                .map(|c| c.sub_candidates.as_slice())
                .unwrap_or(&[])
        } else {
            &[]
        };

        let row_height: f32 = 28.0 * s;
        let popup_h: f32 = if has_active_sub {
            sub_candidates.len() as f32 * row_height + 12.0 * s
        } else {
            0.0
        };

        let max_sub_w = if has_active_sub {
            sub_candidates
                .iter()
                .map(|item| self.measure_text(item, font_size))
                .fold(0.0_f32, f32::max)
        } else {
            0.0
        };
        let popup_w: f32 = (max_sub_w + 48.0 * s).max(190.0 * s);

        let active_item_x = measured_items
            .get(state.selected_index)
            .map(|m| m.start_x)
            .unwrap_or(padding_h);
        let popup_x = active_item_x.max(padding_h);

        let canvas_w = if has_active_sub {
            total_bar_width.max(popup_x + popup_w + padding_h)
        } else {
            total_bar_width
        };
        let canvas_h = if has_active_sub {
            bar_height + 6.0 * s + popup_h
        } else {
            bar_height
        };

        let mut pixmap = Pixmap::new(canvas_w.ceil() as u32, canvas_h.ceil() as u32)?;
        pixmap.fill(tiny_skia::Color::TRANSPARENT);

        draw_card_background_at(
            &mut pixmap,
            &self.theme,
            CardBackgroundParams {
                x: 0.0,
                y: 0.0,
                width: total_bar_width,
                height: bar_height,
                radius: 10.0 * s,
                stroke_width: 1.0 * s,
            },
        );

        // 绘制条目
        let baseline_y = (bar_height + font_size * 0.72) / 2.0;
        for (idx, item) in measured_items.iter().enumerate() {
            let cand = &state.candidates[idx];
            let is_selected = idx == state.selected_index;
            if is_selected {
                let badge_h = 28.0 * s;
                let badge_y = (bar_height - badge_h) / 2.0;
                draw_rounded_rect(
                    &mut pixmap,
                    RoundedRectParams {
                        x: item.start_x,
                        y: badge_y,
                        w: item.total_item_width,
                        h: badge_h,
                        r: 6.0 * s,
                    },
                    self.theme.highlight_badge,
                );

                let text_x = item.start_x + 8.0 * s;
                self.draw_text(
                    &mut pixmap,
                    item.index_str,
                    text_x,
                    baseline_y,
                    font_size * 0.85,
                    self.theme.highlight_text,
                );

                let mut content_x = text_x + item.index_width + 3.0 * s;
                if item.is_expandable {
                    draw_calendar_icon(
                        &mut pixmap,
                        &self.theme,
                        content_x,
                        badge_y + 8.5 * s,
                        true,
                        s,
                    );
                    content_x += item.icon_width;
                }

                self.draw_text(
                    &mut pixmap,
                    &cand.text,
                    content_x,
                    baseline_y,
                    font_size,
                    self.theme.highlight_text,
                );
            } else {
                self.draw_text(
                    &mut pixmap,
                    item.index_str,
                    item.start_x,
                    baseline_y,
                    font_size * 0.85,
                    self.theme.index_text,
                );

                let mut content_x = item.start_x + item.index_width + 4.0 * s;
                if item.is_expandable {
                    let icon_y = (bar_height - 11.0 * s) / 2.0;
                    draw_calendar_icon(&mut pixmap, &self.theme, content_x, icon_y, false, s);
                    content_x += item.icon_width;
                }

                self.draw_text(
                    &mut pixmap,
                    &cand.text,
                    content_x,
                    baseline_y,
                    font_size,
                    self.theme.candidate_text,
                );
            }
        }

        let action_area_width = 24.0 * s;
        let arrow_x = total_bar_width - action_area_width;
        let arrow_center_y = bar_height / 2.0;
        draw_dropdown_arrow(
            &mut pixmap,
            &self.theme,
            arrow_x + 6.0 * s,
            arrow_center_y,
            s,
        );

        if has_active_sub {
            let popup_top_y = bar_height + 6.0 * s;
            draw_card_background_at(
                &mut pixmap,
                &self.theme,
                CardBackgroundParams {
                    x: popup_x,
                    y: popup_top_y,
                    width: popup_w,
                    height: popup_h,
                    radius: 10.0 * s,
                    stroke_width: 1.0 * s,
                },
            );

            for (s_idx, sub_str) in sub_candidates.iter().enumerate() {
                let row_y = popup_top_y + 6.0 * s + s_idx as f32 * row_height;
                let is_sub_selected = s_idx == state.expanded_selected_index();
                let sub_num_str = if s_idx < PRECACHED_INDEX_STRINGS.len() {
                    PRECACHED_INDEX_STRINGS[s_idx]
                } else {
                    "•"
                };

                if is_sub_selected {
                    draw_rounded_rect(
                        &mut pixmap,
                        RoundedRectParams {
                            x: popup_x + 6.0 * s,
                            y: row_y,
                            w: popup_w - 12.0 * s,
                            h: row_height,
                            r: 6.0 * s,
                        },
                        self.theme.highlight_badge,
                    );
                }

                let sub_num_x = popup_x + 14.0 * s;
                let sub_baseline_y = row_y + (row_height + font_size * 0.72) / 2.0;
                let num_color = if is_sub_selected {
                    self.theme.highlight_text
                } else {
                    self.theme.index_text
                };
                self.draw_text(
                    &mut pixmap,
                    sub_num_str,
                    sub_num_x,
                    sub_baseline_y,
                    font_size * 0.85,
                    num_color,
                );

                let sub_text_x = sub_num_x + 14.0 * s;
                let text_color = if is_sub_selected {
                    self.theme.highlight_text
                } else {
                    self.theme.candidate_text
                };
                self.draw_text(
                    &mut pixmap,
                    sub_str,
                    sub_text_x,
                    sub_baseline_y,
                    font_size,
                    text_color,
                );
            }
        }

        Some(pixmap)
    }
}

#[cfg(test)]
mod tests;
