pub mod capsule;
pub mod draw;
pub mod expanded;
pub mod font;
pub mod layout;

use crate::theme::{Color, UiTheme};
use draw::{blend_glyph, draw_rounded_rect, RoundedRectParams};
use expanded::{render_expanded_popup_card, ExpandedCardRenderParams};
pub use font::CachedGlyph;
use font::FontManager;
use fontdue::{Font, FontSettings};
use layout::{measure_candidates_bar, LayoutParams};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tiny_skia::Pixmap;
use waylan_core::CandidateBoxState;

/// 最大字形位图缓存条目数（覆盖绝大多数常用汉字与符号，占用内存不足 1MB）
const MAX_GLYPH_CACHE_SIZE: usize = 2048;

/// 候选框矢量渲染器（1:1 对标现代单行胶囊样式与二级卡片）
pub struct CandidateBoxRenderer {
    pub theme: UiTheme,
    font: Option<Font>,
    /// 回退字体链（用于补充主字体未覆盖的符号、Emoji 及多语言字形）
    pub fallback_fonts: Vec<Font>,
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
            fallback_fonts: Vec::new(),
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

    /// 查询当前是否已加载有效主字体或回退字体
    pub fn has_font(&self) -> bool {
        self.font.is_some() || !self.fallback_fonts.is_empty()
    }

    /// 添加额外的回退字体到字体链末尾
    pub fn add_fallback_font(&mut self, font: Font) {
        self.fallback_fonts.push(font);
        self.clear_glyph_cache();
    }

    /// 寻找能够渲染给定字符的最佳字体（优先主中文字体，若未覆盖则依次查询回退字体链）
    pub fn get_font_for_char(&self, ch: char) -> Option<&Font> {
        if let Some(font) = self.font.as_ref() {
            if font.lookup_glyph_index(ch) != 0 {
                return Some(font);
            }
        }
        for fb in &self.fallback_fonts {
            if fb.lookup_glyph_index(ch) != 0 {
                return Some(fb);
            }
        }
        self.font.as_ref().or_else(|| self.fallback_fonts.first())
    }

    /// 手动从指定文件路径加载字体
    pub fn load_font_from_path(&mut self, path: &Path) -> bool {
        fs::read(path)
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
            Err(e) => {
                log::warn!(
                    "[候选框渲染] 从内存字节流解析字体失败，字节长度: {}，原因: {}",
                    bytes.len(),
                    e
                );
                false
            }
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

    /// 当字形缓存达到上限时，淘汰约 25% 的旧条目避免全量清空带来的重光栅化抖动
    fn evict_cache_if_needed(cache: &mut HashMap<(char, u32), CachedGlyph>) {
        if cache.len() >= MAX_GLYPH_CACHE_SIZE {
            let evict_count = MAX_GLYPH_CACHE_SIZE / 4;
            let keys_to_remove: Vec<(char, u32)> =
                cache.keys().take(evict_count).cloned().collect();
            for key in keys_to_remove {
                cache.remove(&key);
            }
        }
    }

    /// 测量一段文字的排版宽度（单次加锁并顺带预热字形缓存，使后续绘制 100% 命中缓存）
    pub fn measure_text(&self, text: &str, size: f32) -> f32 {
        if !self.has_font() {
            let s = self.scale.max(0.5);
            return text.chars().count() as f32 * (size * 0.8 + 3.0 * s);
        }

        let size_key = (size * 100.0).round() as u32;
        let mut width = 0.0;
        if let Ok(mut cache) = self.glyph_cache.lock() {
            for ch in text.chars() {
                if let Some(cached) = cache.get(&(ch, size_key)) {
                    width += cached.metrics.advance_width;
                } else if let Some(font) = self.get_font_for_char(ch) {
                    let (metrics, bitmap) = font.rasterize(ch, size);
                    width += metrics.advance_width;
                    Self::evict_cache_if_needed(&mut cache);
                    cache.insert(
                        (ch, size_key),
                        CachedGlyph {
                            metrics,
                            bitmap: Arc::from(bitmap),
                        },
                    );
                } else {
                    let s = self.scale.max(0.5);
                    width += size * 0.8 + 3.0 * s;
                }
            }
        } else {
            for ch in text.chars() {
                if let Some(font) = self.get_font_for_char(ch) {
                    width += font.metrics(ch, size).advance_width;
                }
            }
        }
        width
    }

    /// 获取或光栅化指定字符的字形位图
    fn get_or_rasterize_glyph(&self, ch: char, size: f32) -> Option<CachedGlyph> {
        let size_key = (size * 100.0).round() as u32;
        let font = self.get_font_for_char(ch)?;

        if let Ok(mut cache) = self.glyph_cache.lock() {
            if let Some(cached) = cache.get(&(ch, size_key)) {
                return Some(cached.clone());
            }

            let (metrics, bitmap) = font.rasterize(ch, size);
            let cached = CachedGlyph {
                metrics,
                bitmap: Arc::from(bitmap),
            };
            Self::evict_cache_if_needed(&mut cache);
            cache.insert((ch, size_key), cached.clone());
            Some(cached)
        } else {
            None
        }
    }

    /// 加载默认系统字体与回退字体链
    fn load_default_fonts(&mut self) {
        self.font = FontManager::load_default_fonts();
        self.fallback_fonts = FontManager::load_fallback_fonts();
    }

    /// 绘制文本
    pub(crate) fn draw_text(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
    ) {
        if !self.has_font() {
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
                    blend_glyph(pixmap, &glyph, (glyph_x, glyph_y), color);
                }
                current_x += glyph.metrics.advance_width;
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
        let padding_h: f32 = self.theme.padding.max(4.0) * s;
        let corner_radius: f32 = self.theme.corner_radius.max(2.0) * s;
        let item_gap: f32 = 14.0 * s;
        let font_size = self.font_size * s;

        let (measured_items, total_bar_width) = measure_candidates_bar(
            LayoutParams {
                candidates: &state.candidates,
                selected_index: state.selected_index,
                preedit: &state.preedit,
                mode_badge: &state.mode_badge,
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

        let raw_w = if has_active_sub {
            total_bar_width.max(popup_x + popup_w + padding_h)
        } else {
            total_bar_width
        }
        .ceil();
        let raw_h = if has_active_sub {
            bar_height + 6.0 * s + popup_h
        } else {
            bar_height
        }
        .ceil();

        if raw_w > 4096.0 || raw_h > 2048.0 {
            log::warn!(
                "[候选框渲染] 候选框画布尺寸超出上限并进行安全截断，原始尺寸: {}x{}，安全上限: 4096x2048",
                raw_w,
                raw_h
            );
        }

        let canvas_w = raw_w.clamp(1.0, 4096.0) as u32;
        let canvas_h = raw_h.clamp(1.0, 2048.0) as u32;

        if canvas_w == 0 || canvas_h == 0 {
            return None;
        }

        let mut pixmap = Pixmap::new(canvas_w, canvas_h)?;

        capsule::render_capsule_bar(
            self,
            &mut pixmap,
            state,
            capsule::CapsuleBarParams {
                measured_items: &measured_items,
                total_bar_width,
                bar_height,
                padding_h,
                corner_radius,
                font_size,
                scale: s,
            },
        );

        if has_active_sub {
            render_expanded_popup_card(
                self,
                &mut pixmap,
                state,
                sub_candidates,
                ExpandedCardRenderParams {
                    popup_x,
                    popup_top_y: bar_height + 6.0 * s,
                    popup_w,
                    popup_h,
                    row_height,
                    corner_radius,
                    font_size,
                    scale: s,
                },
            );
        }

        Some(pixmap)
    }
}

#[cfg(test)]
mod tests;
