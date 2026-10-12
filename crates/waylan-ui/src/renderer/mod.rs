pub mod capsule;
pub mod draw;
pub mod expanded;
pub mod font;
pub mod grid;
pub mod layout;

use crate::theme::{Color, UiTheme};
use draw::{blend_glyph, draw_rounded_rect, RoundedRectParams};
use expanded::{render_expanded_popup_card, ExpandedCardRenderParams};
pub use font::CachedGlyph;
use font::FontManager;
use fontdue::{Font, FontSettings};
use layout::{measure_candidates_bar, CandidateHitTarget, HitRegion, LayoutParams};
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
    font: Option<Arc<Font>>,
    /// 回退字体链（用于补充主字体未覆盖的符号、Emoji 及多语言字形，通过 Arc 共享避免深拷贝）
    pub fallback_fonts: Arc<Vec<Font>>,
    /// 基础候选字号大小
    pub font_size: f32,
    /// 屏幕缩放因子（HiDPI 支持，默认 1.0）
    pub scale: f32,
    /// 字形位图与度量信息缓存（LRU 策略，避免高频字符每次击键重复矢量光栅化）
    glyph_cache: Mutex<GlyphCacheState>,
    /// 最近一次渲染生成的鼠标可点击命中测试区域列表
    hit_regions: Mutex<Vec<HitRegion>>,
}

const NULL_INDEX: usize = usize::MAX;

/// 双向链表字形缓存节点
struct CacheNode {
    key: (char, u32),
    glyph: CachedGlyph,
    prev: usize,
    next: usize,
}

/// 严格 O(1) 复杂度的数组双向链表字形 LRU 缓存状态机
///
/// # 设计原理
/// - **实现初衷**：消除原实现中达到上限时在互斥锁内部进行 2048 长度向量全量分配与非稳定排序的显著停顿。
/// - **核心优势**：基于固定槽位复用的 Arena 双向链表，查找、命中移动、未命中淘汰均严格为 O(1) 常数时间，零堆内存重分配。
/// - **代价与局限**：每个条目额外占用 16 字节的链表双向指针索引。
struct GlyphCacheState {
    map: HashMap<(char, u32), usize>,
    nodes: Vec<CacheNode>,
    free_list: Vec<usize>,
    head: usize,
    tail: usize,
}

impl GlyphCacheState {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity),
            nodes: Vec::with_capacity(capacity),
            free_list: Vec::new(),
            head: NULL_INDEX,
            tail: NULL_INDEX,
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.map.len()
    }

    fn clear(&mut self) {
        self.map.clear();
        self.nodes.clear();
        self.free_list.clear();
        self.head = NULL_INDEX;
        self.tail = NULL_INDEX;
    }

    /// 从双向链表中解绑指定节点
    #[inline]
    fn detach(&mut self, idx: usize) {
        let prev = self.nodes[idx].prev;
        let next = self.nodes[idx].next;

        if prev != NULL_INDEX {
            self.nodes[prev].next = next;
        } else {
            self.head = next;
        }

        if next != NULL_INDEX {
            self.nodes[next].prev = prev;
        } else {
            self.tail = prev;
        }

        self.nodes[idx].prev = NULL_INDEX;
        self.nodes[idx].next = NULL_INDEX;
    }

    /// 将指定节点挂载至链表尾部（标记为最新访问 MRU）
    #[inline]
    fn attach_tail(&mut self, idx: usize) {
        self.nodes[idx].prev = self.tail;
        self.nodes[idx].next = NULL_INDEX;

        if self.tail != NULL_INDEX {
            self.nodes[self.tail].next = idx;
        } else {
            self.head = idx;
        }

        self.tail = idx;
    }

    /// 查询字形位图并在命中时 O(1) 提升至链表末尾
    fn get(&mut self, key: &(char, u32)) -> Option<&CachedGlyph> {
        let &idx = self.map.get(key)?;
        self.detach(idx);
        self.attach_tail(idx);
        Some(&self.nodes[idx].glyph)
    }

    /// 插入字形位图并在达到容量上限时 O(1) 淘汰最久未使用的头部条目
    fn insert(&mut self, key: (char, u32), glyph: CachedGlyph) {
        if let Some(&idx) = self.map.get(&key) {
            self.nodes[idx].glyph = glyph;
            self.detach(idx);
            self.attach_tail(idx);
            return;
        }

        if self.map.len() >= MAX_GLYPH_CACHE_SIZE {
            let lru_idx = self.head;
            if lru_idx != NULL_INDEX {
                let old_key = self.nodes[lru_idx].key;
                self.map.remove(&old_key);
                self.detach(lru_idx);
                self.nodes[lru_idx].key = key;
                self.nodes[lru_idx].glyph = glyph;
                self.attach_tail(lru_idx);
                self.map.insert(key, lru_idx);
                return;
            }
        }

        let new_idx = if let Some(free_idx) = self.free_list.pop() {
            self.nodes[free_idx] = CacheNode {
                key,
                glyph,
                prev: NULL_INDEX,
                next: NULL_INDEX,
            };
            free_idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(CacheNode {
                key,
                glyph,
                prev: NULL_INDEX,
                next: NULL_INDEX,
            });
            idx
        };

        self.attach_tail(new_idx);
        self.map.insert(key, new_idx);
    }
}

impl CandidateBoxRenderer {
    pub fn new(theme: UiTheme) -> Self {
        let mut renderer = Self {
            theme,
            font: None,
            fallback_fonts: Arc::new(Vec::new()),
            font_size: 15.0,
            scale: 1.0,
            glyph_cache: Mutex::new(GlyphCacheState::with_capacity(256)),
            hit_regions: Mutex::new(Vec::with_capacity(36)),
        };
        renderer.load_default_fonts();
        renderer
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
        self.font.is_some() || !self.fallback_fonts.is_empty()
    }

    /// 添加额外的回退字体到字体链末尾（写入时复制 Arc 内部数组）
    pub fn add_fallback_font(&mut self, font: Font) {
        let mut new_fonts = (*self.fallback_fonts).clone();
        new_fonts.push(font);
        self.fallback_fonts = Arc::new(new_fonts);
        self.clear_glyph_cache();
    }

    /// 寻找能够渲染给定字符的最佳字体（优先主中文字体，若未覆盖或无有效矢量轮廓则依次查询回退字体链）
    pub fn get_font_for_char(&self, ch: char) -> Option<&Font> {
        let has_valid_outline = |font: &Font| -> bool {
            font.lookup_glyph_index(ch) != 0
                && (ch.is_whitespace() || font.metrics(ch, 15.0).width > 0)
        };
        if let Some(font) = self.font.as_deref() {
            if has_valid_outline(font) {
                return Some(font);
            }
        }
        for fb in self.fallback_fonts.iter() {
            if has_valid_outline(fb) {
                return Some(fb);
            }
        }
        self.font.as_deref().or_else(|| self.fallback_fonts.first())
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
                self.font = Some(Arc::new(font));
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

    /// 在已持有缓存锁的情况下获取或光栅化指定字符的字形位图（命中缓存时零字体查询开销）
    fn get_or_rasterize_glyph_locked(
        &self,
        cache: &mut GlyphCacheState,
        ch: char,
        size: f32,
        size_key: u32,
    ) -> Option<CachedGlyph> {
        if let Some(cached) = cache.get(&(ch, size_key)) {
            return Some(cached.clone());
        }

        let font = self.get_font_for_char(ch)?;
        let (metrics, bitmap) = font.rasterize(ch, size);
        let cached = CachedGlyph {
            metrics,
            bitmap: Arc::from(bitmap),
        };
        cache.insert((ch, size_key), cached.clone());
        Some(cached)
    }

    /// 加载默认系统字体与回退字体链
    fn load_default_fonts(&mut self) {
        self.font = FontManager::load_default_fonts();
        self.fallback_fonts = FontManager::load_fallback_fonts();
    }

    /// 绘制文本（单次加锁查询字形缓存，消除逐字符加锁与命中时的重复字体探测）
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
            let (glyph_w, glyph_h) = (size * 0.8, size);
            let placeholder_color =
                Color::from_rgba(color.r, color.g, color.b, (color.a * 0.35).clamp(0.0, 1.0));
            for _ in text.chars() {
                let rect = RoundedRectParams {
                    x: current_x,
                    y: y - glyph_h,
                    w: glyph_w,
                    h: glyph_h,
                    r: 2.0 * s,
                };
                draw_rounded_rect(pixmap, rect, placeholder_color);
                current_x += glyph_w + 3.0 * s;
            }
            return;
        }

        let size_key = (size * 100.0).round() as u32;
        let glyphs: Vec<(CachedGlyph, i32, i32)> = if let Ok(mut cache) = self.glyph_cache.lock() {
            let mut list = Vec::with_capacity(text.len());
            let mut current_x = x;
            for ch in text.chars() {
                if let Some(glyph) =
                    self.get_or_rasterize_glyph_locked(&mut cache, ch, size, size_key)
                {
                    if glyph.metrics.width > 0 && glyph.metrics.height > 0 {
                        let gx = (current_x + glyph.metrics.xmin as f32) as i32;
                        let gy =
                            (y - glyph.metrics.height as f32 - glyph.metrics.ymin as f32) as i32;
                        list.push((glyph.clone(), gx, gy));
                    }
                    current_x += glyph.metrics.advance_width;
                }
            }
            list
        } else {
            Vec::new()
        };

        for (glyph, gx, gy) in glyphs {
            blend_glyph(pixmap, &glyph, (gx, gy), color);
        }
    }

    /// 渲染单行胶囊候选栏及微信键盘风格二级展开浮层卡片
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
