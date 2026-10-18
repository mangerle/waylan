pub mod capsule;
pub mod draw;
pub mod expanded;
pub mod font;
pub mod grid;
pub mod layout;
pub mod measure_cache;

use crate::theme::{Color, UiTheme};
use cosmic_text::{Buffer, FontSystem, Metrics, SwashCache};
use draw::blend_swash_image;
use expanded::{render_expanded_popup_card, ExpandedCardRenderParams};
use font::FontManager;
use layout::{measure_candidates_bar, CandidateHitTarget, HitRegion, ItemMeasure, LayoutParams};
use measure_cache::TextMeasureCache;
use std::path::Path;
use tiny_skia::Pixmap;
use waylan_core::CandidateBoxState;

/// cosmic-text 文本布局与字形光栅化状态机
struct TextEngineState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    buffer: Buffer,
    measure_cache: TextMeasureCache,
}

/// 文本绘制选项参数对象
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrawTextOptions<'a> {
    pub text: &'a str,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: Color,
}

/// 胶囊候选栏与展开浮层卡片几何布局度量结果
struct CandidateBoxGeometry<'a> {
    measured_items: Vec<ItemMeasure<'a>>,
    total_bar_width: f32,
    bar_height: f32,
    padding_h: f32,
    corner_radius: f32,
    font_size: f32,
    scale: f32,
    has_active_sub: bool,
    sub_candidates: &'a [String],
    popup_x: f32,
    popup_w: f32,
    popup_h: f32,
    row_height: f32,
    canvas_w: u32,
    canvas_h: u32,
}

/// 候选框矢量渲染器（1:1 对标现代单行胶囊样式与二级卡片）
pub struct CandidateBoxRenderer {
    pub theme: UiTheme,
    /// 基础候选字号大小
    pub font_size: f32,
    /// 屏幕缩放因子（HiDPI 支持，默认 1.0）
    pub scale: f32,
    /// cosmic-text 文本排版与渲染引擎状态机
    text_engine: TextEngineState,
    /// 最近一次渲染生成的鼠标可点击命中测试区域列表
    hit_regions: Vec<HitRegion>,
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
            text_engine: TextEngineState {
                font_system,
                swash_cache,
                buffer,
                measure_cache: TextMeasureCache::new(),
            },
            hit_regions: Vec::with_capacity(36),
        }
    }

    /// 根据相对候选框表面左上角的物理像素坐标 (x, y) 执行命中测试，返回命中的操作目标
    pub fn hit_test(&self, x: f32, y: f32) -> Option<CandidateHitTarget> {
        for region in &self.hit_regions {
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
        self.text_engine.font_system.db().faces().count() > 0
    }

    /// 手动从指定文件路径加载字体
    pub fn load_font_from_path(&mut self, path: &Path) -> bool {
        let res = self
            .text_engine
            .font_system
            .db_mut()
            .load_font_file(path)
            .is_ok();
        if res {
            self.clear_glyph_cache();
        }
        res
    }

    /// 从内存字节流中加载字体
    pub fn load_font_from_bytes(&mut self, bytes: &[u8]) -> bool {
        self.text_engine
            .font_system
            .db_mut()
            .load_font_data(bytes.to_vec());
        self.clear_glyph_cache();
        true
    }

    /// 获取当前字体数据库中已加载的字体变体条目总数（用于诊断与测试）
    pub fn glyph_cache_len(&self) -> usize {
        self.text_engine.font_system.db().faces().count()
    }

    /// 清空字形光栅化与排版测量缓存
    pub fn clear_glyph_cache(&mut self) {
        self.text_engine.swash_cache = SwashCache::new();
        self.text_engine.measure_cache.clear();
    }

    /// 测量一段文字的排版宽度（基于 cosmic-text 高级塑形与布局，复用 Buffer 与直接映射缓存消除多余塑形）
    pub fn measure_text(&mut self, text: &str, size: f32) -> f32 {
        self.text_engine
            .measure_cache
            .ensure_shaped(
                &mut self.text_engine.buffer,
                &mut self.text_engine.font_system,
                text,
                size,
            )
            .width
    }

    /// 绘制文本（基线对齐，支持复杂连字、特殊符号与多色 Emoji，使用参数对象收敛入参）
    pub(crate) fn draw_text(&mut self, pixmap: &mut Pixmap, opts: DrawTextOptions<'_>) {
        let DrawTextOptions {
            text,
            x,
            y,
            size,
            color,
        } = opts;
        let entry = self.text_engine.measure_cache.ensure_shaped(
            &mut self.text_engine.buffer,
            &mut self.text_engine.font_system,
            text,
            size,
        );

        let x_round = x.round() as i32;
        let y_round = y.round() as i32;
        for glyph in &entry.glyphs {
            let px = x_round + glyph.x;
            let py = y_round + glyph.y;
            if let Some(img) = self
                .text_engine
                .swash_cache
                .get_image(&mut self.text_engine.font_system, glyph.cache_key)
            {
                if img.placement.width > 0 && img.placement.height > 0 {
                    let gx = px + img.placement.left;
                    let gy = py - img.placement.top;
                    blend_swash_image(pixmap, img, (gx, gy), color);
                }
            }
        }
    }

    /// 计算胶囊候选栏与展开浮层的几何尺寸与画布规格
    fn calculate_geometry<'a>(
        &mut self,
        state: &'a CandidateBoxState,
    ) -> Option<CandidateBoxGeometry<'a>> {
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

        Some(CandidateBoxGeometry {
            measured_items,
            total_bar_width,
            bar_height,
            padding_h,
            corner_radius,
            font_size,
            scale: s,
            has_active_sub,
            sub_candidates,
            popup_x,
            popup_w,
            popup_h,
            row_height,
            canvas_w,
            canvas_h,
        })
    }

    /// 渲染单行胶囊候选栏及二级展开浮层卡片
    pub fn render(&mut self, state: &CandidateBoxState) -> Option<Pixmap> {
        if state.candidates.is_empty() && !state.is_voice_active() {
            return None;
        }

        // 若处于语音听写状态，优先渲染语音流式胶囊波形栏
        if state.is_voice_active() {
            let mut regions = Vec::with_capacity(4);
            let pix = capsule::render_voice_capsule_bar(self, state, &mut regions);
            self.hit_regions = regions;
            return pix;
        }

        // 若处于 5 行网格大面板展开状态，优先渲染网格面板
        if state.is_grid_expanded {
            let mut regions = Vec::with_capacity(36);
            let pix = grid::render_grid_expanded_panel(self, state, &mut regions);
            self.hit_regions = regions;
            return pix;
        }

        let geom = self.calculate_geometry(state)?;
        let mut pixmap = Pixmap::new(geom.canvas_w, geom.canvas_h)?;

        let mut regions = capsule::render_capsule_bar(
            self,
            &mut pixmap,
            state,
            capsule::CapsuleBarParams {
                measured_items: &geom.measured_items,
                total_bar_width: geom.total_bar_width,
                bar_height: geom.bar_height,
                padding_h: geom.padding_h,
                corner_radius: geom.corner_radius,
                font_size: geom.font_size,
                scale: geom.scale,
            },
        );

        if geom.has_active_sub {
            render_expanded_popup_card(
                self,
                &mut pixmap,
                &mut regions,
                ExpandedCardRenderParams {
                    state,
                    sub_candidates: geom.sub_candidates,
                    popup_x: geom.popup_x,
                    popup_top_y: geom.bar_height + 6.0 * geom.scale,
                    popup_w: geom.popup_w,
                    popup_h: geom.popup_h,
                    row_height: geom.row_height,
                    corner_radius: geom.corner_radius,
                    font_size: geom.font_size,
                    scale: geom.scale,
                },
            );
        }

        self.hit_regions = regions;
        Some(pixmap)
    }
}

#[cfg(test)]
mod tests;
