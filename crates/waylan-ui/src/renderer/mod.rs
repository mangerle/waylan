pub mod capsule;
pub mod draw;
pub mod expanded;
pub mod font;
mod geometry;
mod glyph_blend;
pub mod grid;
mod grid_layout;
pub mod layout;
pub mod measure_cache;
mod snapshot;
pub mod style;

use crate::theme::{Color, UiTheme};
use cosmic_text::{Buffer, FontSystem, Metrics, SwashCache};
use expanded::{ExpandedCardRenderParams, render_expanded_popup_card};
use font::FontManager;
use glyph_blend::blend_swash_image;
use layout::{CandidateHitTarget, HitRegion, format_candidate_text};
use measure_cache::{TextMeasureCache, TextShapeParams};
use snapshot::{LastRenderSnapshot, compute_state_fingerprint, compute_theme_hash};
use std::path::Path;
use style::RendererStyle;
use tiny_skia::Pixmap;
use waylan_core::config::UiConfig;
use waylan_core::{Candidate, CandidateBoxState, CandidateBoxView, ThemeMode, VoiceBoxState};

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

/// 候选框矢量渲染器（1:1 对标现代单行胶囊样式与二级卡片）
pub struct CandidateBoxRenderer {
    pub theme: UiTheme,
    /// 基础候选字号大小
    pub font_size: f32,
    /// 屏幕缩放因子（HiDPI 支持，默认 1.0）
    pub scale: f32,
    /// 用户配置驱动的排版、主题与序号样式
    pub style: RendererStyle,
    /// cosmic-text 文本排版与渲染引擎状态机
    text_engine: TextEngineState,
    /// 最近一次渲染生成的鼠标可点击命中测试区域列表
    hit_regions: Vec<HitRegion>,
    /// 复用的 Pixmap 像素缓冲区，避免高频渲染反复分配整块画布内存
    scratch_pixmap_buf: Vec<u8>,
    /// 最近一次完整渲染快照（状态与样式均未变时直接复用缓冲，跳过重绘）
    last_render_snapshot: Option<LastRenderSnapshot>,
}

impl CandidateBoxRenderer {
    /// 使用指定主题与默认排版样式创建候选框渲染器
    pub fn new(theme: UiTheme) -> Self {
        Self::with_style(theme, RendererStyle::default())
    }

    /// 使用外部样式参数创建渲染器
    pub fn with_style(theme: UiTheme, style: RendererStyle) -> Self {
        let mut font_system = FontManager::create_font_system();
        let swash_cache = SwashCache::new();
        let buffer = Buffer::new(&mut font_system, Metrics::new(15.0, 18.0));

        Self {
            theme,
            font_size: 15.0,
            scale: 1.0,
            style,
            text_engine: TextEngineState {
                font_system,
                swash_cache,
                buffer,
                measure_cache: TextMeasureCache::new(),
            },
            hit_regions: Vec::with_capacity(36),
            scratch_pixmap_buf: Vec::new(),
            last_render_snapshot: None,
        }
    }

    /// 应用用户界面配置（主题模式、色彩预设、字体、密度、间距、圆角、透明度与序号风格）
    pub fn apply_ui_config(&mut self, ui: &UiConfig, theme_mode: ThemeMode) {
        let font_size_changed = (self.font_size - ui.font_size).abs() > 0.001;
        self.font_size = ui.font_size;
        self.style = RendererStyle::from(ui);
        self.theme = UiTheme::from_config(theme_mode, ui.candidate_theme);
        let opacity = (self.style.opacity.clamp(70, 100) as f32) / 100.0;
        self.theme.background.a = (self.theme.background.a * opacity).clamp(0.0, 1.0);
        if font_size_changed {
            self.clear_glyph_cache();
        } else {
            self.last_render_snapshot = None;
        }
    }

    /// 根据当前序号风格生成候选序号前缀
    pub fn index_label(&self, index: usize) -> String {
        waylan_core::format_candidate_index(self.style.index_style, index)
    }

    /// 生成候选词的最终展示文本（按配置决定是否附加注释）
    pub(crate) fn display_text<'a>(&self, candidate: &'a Candidate) -> std::borrow::Cow<'a, str> {
        format_candidate_text(candidate, self.style.show_pinyin_comment)
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
        self.last_render_snapshot = None;
    }

    /// 测量一段文字的排版宽度（基于 cosmic-text 高级塑形与布局，复用 Buffer 与直接映射缓存消除多余塑形）
    pub fn measure_text(&mut self, text: &str, size: f32) -> f32 {
        self.text_engine
            .measure_cache
            .ensure_shaped(
                &mut self.text_engine.buffer,
                &mut self.text_engine.font_system,
                &TextShapeParams { text, size },
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
            &TextShapeParams { text, size },
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
                && img.placement.width > 0
                && img.placement.height > 0
            {
                let gx = px + img.placement.left;
                let gy = py - img.placement.top;
                blend_swash_image(pixmap, img, (gx, gy), color);
            }
        }
    }

    /// 按给定尺寸分配或复用底层缓冲区的透明 Pixmap 画布
    pub(crate) fn allocate_pixmap(&mut self, width: u32, height: u32) -> Option<Pixmap> {
        let required = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        let size = tiny_skia::IntSize::from_wh(width, height)?;
        let mut buf = std::mem::take(&mut self.scratch_pixmap_buf);
        buf.clear();
        buf.resize(required, 0);
        Pixmap::from_vec(buf, size)
    }

    /// 渲染当前候选框状态并将底层像素缓冲回收到渲染器中复用，返回 `(宽, 高, RGBA 切片)`
    ///
    /// 若输入状态与渲染样式参数均与上次一致，则直接复用已有像素缓冲，跳过全量清零与矢量重绘。
    pub fn render_rgba(&mut self, state: &CandidateBoxState) -> Option<(u32, u32, &[u8])> {
        let font_size_bits = self.font_size.to_bits();
        let scale_bits = self.scale.to_bits();
        let state_hash = compute_state_fingerprint(state);
        let theme_hash = compute_theme_hash(&self.theme);
        let has_time_animation = capsule::has_active_time_animation(state.voice.as_ref());
        if !has_time_animation
            && let Some(snap) = self.last_render_snapshot
            && snap.font_size_bits == font_size_bits
            && snap.scale_bits == scale_bits
            && snap.theme_hash == theme_hash
            && snap.style == self.style
            && snap.state_hash == state_hash
            && let Some(required) = (snap.width as usize)
                .checked_mul(snap.height as usize)
                .and_then(|px| px.checked_mul(4))
            && self.scratch_pixmap_buf.len() >= required
        {
            return Some((
                snap.width,
                snap.height,
                &self.scratch_pixmap_buf[..required],
            ));
        }

        let Some(pixmap) = self.render(state) else {
            self.last_render_snapshot = None;
            return None;
        };
        let width = pixmap.width();
        let height = pixmap.height();
        let required = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        self.scratch_pixmap_buf = pixmap.take();
        self.last_render_snapshot = Some(LastRenderSnapshot {
            state_hash,
            theme_hash,
            style: self.style,
            font_size_bits,
            scale_bits,
            width,
            height,
        });
        Some((width, height, &self.scratch_pixmap_buf[..required]))
    }

    /// 获取最近一次 `render_rgba` 产出的底层像素只读切片（用于避免调用方不相交字段借用冲突）
    #[inline]
    pub fn last_rgba_slice(&self, required_bytes: usize) -> Option<&[u8]> {
        self.scratch_pixmap_buf.get(..required_bytes)
    }

    /// 渲染单行胶囊候选栏及二级展开浮层卡片
    pub fn render(&mut self, state: &CandidateBoxState) -> Option<Pixmap> {
        match state.view() {
            CandidateBoxView::Hidden => None,
            CandidateBoxView::VoiceCapsule(voice) => self.render_voice_capsule(voice),
            CandidateBoxView::GridExpanded {
                grid_candidates,
                selected_index,
            } => self.render_grid_expanded(grid_candidates, selected_index),
            CandidateBoxView::Compact { .. } | CandidateBoxView::ExpandedCard { .. } => {
                self.render_capsule_with_popup(state)
            }
        }
    }

    /// 渲染语音听写胶囊栏并同步更新命中区域
    fn render_voice_capsule(&mut self, voice: &VoiceBoxState) -> Option<Pixmap> {
        let mut regions = Vec::with_capacity(4);
        let pix = capsule::render_voice_capsule_bar(self, voice, &mut regions);
        self.hit_regions = regions;
        pix
    }

    /// 渲染 5 行大网格候选面板并同步更新命中区域
    fn render_grid_expanded(
        &mut self,
        grid_candidates: &[Candidate],
        selected_index: usize,
    ) -> Option<Pixmap> {
        let mut regions = Vec::with_capacity(36);
        let pix =
            grid::render_grid_expanded_panel(self, grid_candidates, selected_index, &mut regions);
        self.hit_regions = regions;
        pix
    }

    /// 渲染单行胶囊候选栏，并在存在活动子候选时叠加二级展开浮层卡片
    fn render_capsule_with_popup(&mut self, state: &CandidateBoxState) -> Option<Pixmap> {
        let geom = self.calculate_geometry(state)?;
        let mut pixmap = self.allocate_pixmap(geom.canvas_w, geom.canvas_h)?;

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
