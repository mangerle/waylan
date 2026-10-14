pub mod capsule;
pub mod draw;
pub mod expanded;
pub mod font;
pub mod grid;
pub mod layout;

use crate::theme::{Color, UiTheme};
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache};
use draw::blend_swash_image;
use expanded::{render_expanded_popup_card, ExpandedCardRenderParams};
use font::FontManager;
use layout::{measure_candidates_bar, CandidateHitTarget, HitRegion, LayoutParams};
use std::path::Path;
use std::sync::Mutex;
use tiny_skia::Pixmap;
use waylan_core::CandidateBoxState;

/// cosmic-text 文本布局与字形光栅化状态机
struct TextEngineState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    buffer: Buffer,
}

/// 候选框矢量渲染器（1:1 对标现代单行胶囊样式与二级卡片）
pub struct CandidateBoxRenderer {
    pub theme: UiTheme,
    /// 基础候选字号大小
    pub font_size: f32,
    /// 屏幕缩放因子（HiDPI 支持，默认 1.0）
    pub scale: f32,
    /// cosmic-text 文本排版与渲染引擎状态机
    text_engine: Mutex<TextEngineState>,
    /// 最近一次渲染生成的鼠标可点击命中测试区域列表
    hit_regions: Mutex<Vec<HitRegion>>,
}

impl CandidateBoxRenderer {
    pub fn new(theme: UiTheme) -> Self {
        let mut font_system = FontManager::create_font_system();
        let swash_cache = SwashCache::new();
        let buffer = Buffer::new(&mut font_system, Metrics::new(15.0, 18.0));

        Self {
            theme,
            font_size: 15.0,
            scale: 1.0,
            text_engine: Mutex::new(TextEngineState {
                font_system,
                swash_cache,
                buffer,
            }),
            hit_regions: Mutex::new(Vec::with_capacity(36)),
        }
    }

    /// 根据相对候选框表面左上角的物理像素坐标 (x, y) 执行命中测试，返回命中的操作目标
    pub fn hit_test(&self, x: f32, y: f32) -> Option<CandidateHitTarget> {
        let lock = self.hit_regions.lock().ok()?;
        for region in lock.iter() {
            if region.contains(x, y) {
                return Some(region.target);
            }
        }
        None
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
        self.text_engine
            .lock()
            .map(|e| e.font_system.db().faces().count() > 0)
            .unwrap_or(false)
    }

    /// 手动从指定文件路径加载字体
    pub fn load_font_from_path(&mut self, path: &Path) -> bool {
        if let Ok(mut engine) = self.text_engine.lock() {
            let res = engine.font_system.db_mut().load_font_file(path).is_ok();
            if res {
                self.clear_glyph_cache();
            }
            res
        } else {
            false
        }
    }

    /// 从内存字节流中加载字体
    pub fn load_font_from_bytes(&mut self, bytes: &[u8]) -> bool {
        if let Ok(mut engine) = self.text_engine.lock() {
            engine.font_system.db_mut().load_font_data(bytes.to_vec());
            self.clear_glyph_cache();
            true
        } else {
            false
        }
    }

    /// 获取当前字形缓存条目总数（用于诊断与测试）
    pub fn glyph_cache_len(&self) -> usize {
        self.text_engine
            .lock()
            .map(|e| e.font_system.db().faces().count())
            .unwrap_or(0)
    }

    /// 清空字形光栅化缓存
    pub fn clear_glyph_cache(&self) {
        if let Ok(mut engine) = self.text_engine.lock() {
            engine.swash_cache = SwashCache::new();
        }
    }

    /// 测量一段文字的排版宽度（基于 cosmic-text 高级塑形与布局，复用 Buffer 消除堆分配）
    pub fn measure_text(&self, text: &str, size: f32) -> f32 {
        let Ok(mut engine) = self.text_engine.lock() else {
            let s = self.scale.max(0.5);
            return text.chars().count() as f32 * (size * 0.8 + 3.0 * s);
        };
        let engine = &mut *engine;
        engine.buffer.set_metrics(Metrics::new(size, size * 1.2));
        engine.buffer.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        engine
            .buffer
            .shape_until_scroll(&mut engine.font_system, false);
        let mut width = 0.0f32;
        for run in engine.buffer.layout_runs() {
            width = width.max(run.line_w);
        }
        width
    }

    /// 绘制文本（基线对齐，支持复杂连字、特殊符号与多色 Emoji）
    pub(crate) fn draw_text(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
    ) {
        let Ok(mut engine) = self.text_engine.lock() else {
            return;
        };
        let engine = &mut *engine;
        engine.buffer.set_metrics(Metrics::new(size, size * 1.2));
        engine.buffer.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        engine
            .buffer
            .shape_until_scroll(&mut engine.font_system, false);

        for run in engine.buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((x, y - run.line_y), 1.0);
                if let Some(img) = engine
                    .swash_cache
                    .get_image(&mut engine.font_system, physical.cache_key)
                {
                    if img.placement.width > 0 && img.placement.height > 0 {
                        let gx = physical.x + img.placement.left;
                        let gy = physical.y - img.placement.top;
                        blend_swash_image(pixmap, img, (gx, gy), color);
                    }
                }
            }
        }
    }

    /// 渲染单行胶囊候选栏及二级展开浮层卡片
    pub fn render(&self, state: &CandidateBoxState) -> Option<Pixmap> {
        if state.candidates.is_empty() {
            return None;
        }

        // 若处于 5 行网格大面板展开状态，优先渲染网格面板
        if state.is_grid_expanded {
            let mut regions = Vec::with_capacity(36);
            let pix = grid::render_grid_expanded_panel(self, state, &mut regions);
            if let Ok(mut lock) = self.hit_regions.lock() {
                *lock = regions;
            }
            return pix;
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

        let align = (s.round() as u32).max(1);
        let clamped_w = raw_w.clamp(1.0, 4096.0) as u32;
        let clamped_h = raw_h.clamp(1.0, 2048.0) as u32;
        let canvas_w = clamped_w.div_ceil(align).saturating_mul(align);
        let canvas_h = clamped_h.div_ceil(align).saturating_mul(align);

        if canvas_w == 0 || canvas_h == 0 {
            return None;
        }

        let mut pixmap = Pixmap::new(canvas_w, canvas_h)?;

        let mut regions = capsule::render_capsule_bar(
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
                &mut regions,
                ExpandedCardRenderParams {
                    state,
                    sub_candidates,
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

        if let Ok(mut lock) = self.hit_regions.lock() {
            *lock = regions;
        }

        Some(pixmap)
    }
}

#[cfg(test)]
mod tests;
