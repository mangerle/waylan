use crate::theme::{Color, UiTheme};
use fontdue::{Font, FontSettings};
use log::{info, warn};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
use waylan_core::CandidateBoxState;

/// 最大字形位图缓存条目数（覆盖绝大多数常用汉字与符号，占用内存不足 1MB）
const MAX_GLYPH_CACHE_SIZE: usize = 2048;

/// 缓存的单字形光栅化位图与度量信息
#[derive(Debug, Clone)]
pub struct CachedGlyph {
    pub metrics: fontdue::Metrics,
    pub bitmap: Vec<u8>,
}

/// 候选框矢量渲染器（1:1 对标现代单行胶囊样式）
pub struct CandidateBoxRenderer {
    pub theme: UiTheme,
    font: Option<Font>,
    /// 基础候选字号大小
    pub font_size: f32,
    /// 字形位图与度量信息缓存（避免高频字符每次击键重复矢量光栅化）
    glyph_cache: Mutex<HashMap<(char, u32), CachedGlyph>>,
}

/// 圆角矩形几何参数
struct RoundedRectParams {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
}

/// 字形位图绘制参数
struct GlyphBitmapParams<'a> {
    bitmap: &'a [u8],
    x: i32,
    y: i32,
    width: usize,
    height: usize,
}

impl CandidateBoxRenderer {
    pub fn new(theme: UiTheme) -> Self {
        let mut renderer = Self {
            theme,
            font: None,
            font_size: 15.0,
            glyph_cache: Mutex::new(HashMap::with_capacity(256)),
        };
        renderer.load_default_fonts();
        renderer
    }

    /// 查询当前是否已加载有效字体
    pub fn has_font(&self) -> bool {
        self.font.is_some()
    }

    /// 手动从指定文件路径加载字体
    pub fn load_font_from_path(&mut self, path: &Path) -> bool {
        if let Ok(data) = fs::read(path) {
            self.load_font_from_bytes(&data)
        } else {
            false
        }
    }

    /// 从内存字节流中加载字体
    pub fn load_font_from_bytes(&mut self, bytes: &[u8]) -> bool {
        if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
            self.font = Some(font);
            self.clear_glyph_cache();
            true
        } else {
            false
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

    /// 获取字形位图与度量（优先从缓存中获取，未命中则触发矢量光栅化并写入缓存）
    fn get_or_rasterize_glyph(&self, ch: char, size: f32) -> Option<CachedGlyph> {
        let font = self.font.as_ref()?;
        let size_key = (size * 100.0).round() as u32;

        if let Ok(cache) = self.glyph_cache.lock() {
            if let Some(cached) = cache.get(&(ch, size_key)) {
                return Some(cached.clone());
            }
        }

        // 未命中：执行 CPU 矢量光栅化并写入缓存
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

    /// 获取平台默认的字体检索路径列表（覆盖 Ubuntu/Debian/Arch/Fedora/openSUSE/WSL）
    pub fn default_font_search_paths() -> Vec<std::path::PathBuf> {
        let mut paths = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            let p_home = std::path::PathBuf::from(home);
            paths.push(p_home.join(".local/share/fonts/NotoSansCJK-Regular.ttc"));
            paths.push(p_home.join(".local/share/fonts/NotoSansSC-Regular.otf"));
            paths.push(p_home.join(".local/share/fonts/SourceHanSansCN-Regular.otf"));
            paths.push(p_home.join(".fonts/NotoSansCJK-Regular.ttc"));
            paths.push(p_home.join(".fonts/wqy-microhei.ttc"));
        }

        let system_paths = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-sans-cjk-vf/NotoSansCJK-VF.ttf.ttc",
            "/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/wenquanyi/wqy-zenhei/wqy-zenhei.ttc",
            "/usr/share/fonts/wqy-microhei-fonts/wqy-microhei.ttc",
            "/usr/share/fonts/wqy-zenhei-fonts/wqy-zenhei.ttc",
            "/usr/share/fonts/adobe-source-han-sans/SourceHanSansCN-Regular.otf",
            "/usr/share/fonts/truetype/SourceHanSansCN-Regular.otf",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/mnt/c/Windows/Fonts/msyh.ttc",
            "/mnt/c/Windows/Fonts/simhei.ttf",
            "/mnt/c/Windows/Fonts/simsun.ttc",
            "/mnt/c/Windows/Fonts/arial.ttf",
        ];

        for p in &system_paths {
            paths.push(std::path::PathBuf::from(p));
        }
        paths
    }

    /// 在常见字体根目录下进行浅层探测扫描以寻找任意可用字体
    fn scan_fallback_fonts() -> Option<Font> {
        let font_dirs = ["/usr/share/fonts", "/usr/local/share/fonts"];
        for dir in &font_dirs {
            let path = Path::new(dir);
            if !path.is_dir() {
                continue;
            }
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() {
                        if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                            if matches!(ext, "ttf" | "otf" | "ttc") {
                                if let Ok(data) = fs::read(&p) {
                                    if let Ok(font) =
                                        Font::from_bytes(data, FontSettings::default())
                                    {
                                        info!("[矢量渲染] 降级匹配成功载入字体: {:?}", p);
                                        return Some(font);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// 尝试按优先级加载本地中文字体
    fn load_default_fonts(&mut self) {
        let candidate_paths = Self::default_font_search_paths();

        for path in &candidate_paths {
            if path.exists() {
                if let Ok(data) = fs::read(path) {
                    if let Ok(font) = Font::from_bytes(data, FontSettings::default()) {
                        info!("[矢量渲染] 成功载入系统字体: {:?}", path);
                        self.font = Some(font);
                        return;
                    }
                }
            }
        }

        // 降级兜底：若预设字体全未命中，尝试在常见系统字体目录中进行浅层匹配扫描
        if let Some(font) = Self::scan_fallback_fonts() {
            self.font = Some(font);
            return;
        }

        warn!("[矢量渲染] 警告：系统中未检测到任何中西文字体，已自动启用豆腐块几何占位降级模式。建议安装 noto-fonts-cjk 或 wqy-microhei。");
    }

    /// 渲染单行胶囊候选栏
    /// 渲染单行胶囊候选栏及微信键盘风格二级展开浮层卡片
    pub fn render(&self, state: &CandidateBoxState) -> Option<Pixmap> {
        if state.candidates.is_empty() {
            return None;
        }

        let bar_height: f32 = 38.0;
        let padding_h: f32 = 8.0;
        let item_gap: f32 = 14.0;

        // 1. 预先测量计算每个候选词条的宽度
        #[allow(dead_code)]
        struct ItemMeasure {
            index_str: String,
            text_str: String,
            index_width: f32,
            text_width: f32,
            is_expandable: bool,
            icon_width: f32,
            total_item_width: f32,
            start_x: f32,
        }

        let mut measured_items = Vec::new();
        let mut current_pos_x = padding_h + 2.0;

        for (idx, cand) in state.candidates.iter().enumerate() {
            let index_str = format!("{}", idx + 1);
            let text_str = cand.text.clone();

            let index_width = self.measure_text(&index_str, self.font_size * 0.85);
            let text_width = self.measure_text(&text_str, self.font_size);
            let icon_width = if cand.is_expandable { 16.0 } else { 0.0 };

            let is_selected = idx == state.selected_index;
            let item_width = if is_selected {
                // 首选/高亮项是胶囊背景，左右有内边距
                index_width + icon_width + text_width + 16.0
            } else {
                index_width + 4.0 + icon_width + text_width
            };

            measured_items.push(ItemMeasure {
                index_str,
                text_str,
                index_width,
                text_width,
                is_expandable: cand.is_expandable,
                icon_width,
                total_item_width: item_width,
                start_x: current_pos_x,
            });

            current_pos_x += item_width + item_gap;
        }

        // 右侧操作区宽度（展开箭头下拉按钮）
        let action_area_width = 24.0;
        let total_bar_width = current_pos_x - item_gap + action_area_width + padding_h;

        // 2. 测量计算二级扩展卡片几何尺寸（如果当前处于展开状态）
        let active_cand = state.candidates.get(state.selected_index);
        let has_active_sub = state.is_expanded
            && active_cand
                .map(|c| c.is_expandable && !c.sub_candidates.is_empty())
                .unwrap_or(false);

        let sub_candidates = if has_active_sub {
            active_cand.map(|c| &c.sub_candidates).unwrap()
        } else {
            &Vec::new()
        };

        let row_height: f32 = 28.0;
        let popup_h: f32 = if has_active_sub {
            sub_candidates.len() as f32 * row_height + 12.0
        } else {
            0.0
        };

        let max_sub_w = if has_active_sub {
            sub_candidates
                .iter()
                .map(|s| self.measure_text(s, self.font_size))
                .fold(0.0_f32, f32::max)
        } else {
            0.0
        };
        let popup_w: f32 = (max_sub_w + 48.0).max(190.0);

        // 卡片 X 锚点对齐当前选中的扩展项，且限制不超出边界
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
            bar_height + 6.0 + popup_h
        } else {
            bar_height
        };

        let mut pixmap = Pixmap::new(canvas_w.ceil() as u32, canvas_h.ceil() as u32)?;
        pixmap.fill(tiny_skia::Color::TRANSPARENT);

        // 3. 绘制主单行卡片底框（白色胶囊，圆角 12px）
        self.draw_card_background_at(&mut pixmap, 0.0, 0.0, total_bar_width, bar_height);

        // 4. 逐个绘制候选词条目
        let baseline_y = bar_height * 0.65;

        for (idx, item) in measured_items.iter().enumerate() {
            let is_selected = idx == state.selected_index;

            if is_selected {
                // 绘制首选项的高亮翡翠绿胶囊背景（圆角 7px，上下居中）
                let badge_h = 28.0;
                let badge_y = (bar_height - badge_h) / 2.0;
                self.draw_rounded_rect(
                    &mut pixmap,
                    RoundedRectParams {
                        x: item.start_x,
                        y: badge_y,
                        w: item.total_item_width,
                        h: badge_h,
                        r: 7.0,
                    },
                    self.theme.highlight_badge,
                );

                // 白色数字序号
                let text_x = item.start_x + 7.0;
                self.draw_text(
                    &mut pixmap,
                    &item.index_str,
                    text_x,
                    baseline_y,
                    self.font_size * 0.9,
                    self.theme.highlight_text,
                );

                let mut content_x = text_x + item.index_width + 3.0;
                // 若为可展开项，在序号后绘制微型矢量日历图标
                if item.is_expandable {
                    self.draw_calendar_icon(&mut pixmap, content_x, badge_y + 8.5, true);
                    content_x += item.icon_width;
                }

                // 白色汉字候选
                self.draw_text(
                    &mut pixmap,
                    &item.text_str,
                    content_x,
                    baseline_y,
                    self.font_size,
                    self.theme.highlight_text,
                );
            } else {
                // 普通项：灰色数字序号
                self.draw_text(
                    &mut pixmap,
                    &item.index_str,
                    item.start_x,
                    baseline_y,
                    self.font_size * 0.85,
                    self.theme.index_text,
                );

                let mut content_x = item.start_x + item.index_width + 4.0;
                // 若为可展开项，绘制常规色微型矢量日历图标
                if item.is_expandable {
                    let icon_y = (bar_height - 11.0) / 2.0;
                    self.draw_calendar_icon(&mut pixmap, content_x, icon_y, false);
                    content_x += item.icon_width;
                }

                // 深色汉字
                self.draw_text(
                    &mut pixmap,
                    &item.text_str,
                    content_x,
                    baseline_y,
                    self.font_size,
                    self.theme.candidate_text,
                );
            }
        }

        // 5. 绘制右侧下拉箭头 'v'
        let arrow_x = total_bar_width - action_area_width;
        let arrow_center_y = bar_height / 2.0;
        self.draw_dropdown_arrow(&mut pixmap, arrow_x + 6.0, arrow_center_y);

        // 6. 绘制微信键盘同款二级纵向展开浮层卡片
        if has_active_sub {
            let popup_top_y = bar_height + 6.0;
            self.draw_card_background_at(&mut pixmap, popup_x, popup_top_y, popup_w, popup_h);

            for (s_idx, sub_str) in sub_candidates.iter().enumerate() {
                let row_y = popup_top_y + 6.0 + s_idx as f32 * row_height;
                let is_sub_selected = s_idx == state.expanded_selected_index;
                let sub_num_str = format!("{}", s_idx + 1);

                if is_sub_selected {
                    // 翠绿高亮圆角胶囊行背景
                    self.draw_rounded_rect(
                        &mut pixmap,
                        RoundedRectParams {
                            x: popup_x + 4.0,
                            y: row_y,
                            w: popup_w - 8.0,
                            h: 24.0,
                            r: 6.0,
                        },
                        self.theme.highlight_badge,
                    );

                    // 白色序号 + 白色文本
                    let num_x = popup_x + 10.0;
                    let text_baseline = row_y + 17.0;
                    self.draw_text(
                        &mut pixmap,
                        &sub_num_str,
                        num_x,
                        text_baseline,
                        self.font_size * 0.9,
                        self.theme.highlight_text,
                    );
                    let num_w = self.measure_text(&sub_num_str, self.font_size * 0.9);
                    self.draw_text(
                        &mut pixmap,
                        sub_str,
                        num_x + num_w + 8.0,
                        text_baseline,
                        self.font_size * 0.95,
                        self.theme.highlight_text,
                    );
                } else {
                    // 常规行：浅灰序号 + 深色格式文本
                    let num_x = popup_x + 10.0;
                    let text_baseline = row_y + 17.0;
                    self.draw_text(
                        &mut pixmap,
                        &sub_num_str,
                        num_x,
                        text_baseline,
                        self.font_size * 0.85,
                        self.theme.index_text,
                    );
                    let num_w = self.measure_text(&sub_num_str, self.font_size * 0.85);
                    self.draw_text(
                        &mut pixmap,
                        sub_str,
                        num_x + num_w + 8.0,
                        text_baseline,
                        self.font_size * 0.95,
                        self.theme.candidate_text,
                    );
                }
            }
        }

        Some(pixmap)
    }

    /// 绘制精美小巧的微型矢量日历图标（严格纯矢量，不使用Emoji）
    fn draw_calendar_icon(&self, pixmap: &mut Pixmap, x: f32, y: f32, is_selected: bool) {
        let stroke_color = if is_selected {
            self.theme.highlight_text
        } else {
            self.theme.index_text
        };

        // 1. 日历外框 (宽 12px, 高 11px, 圆角 2px)
        self.draw_rounded_rect(
            pixmap,
            RoundedRectParams {
                x,
                y,
                w: 12.0,
                h: 11.0,
                r: 2.0,
            },
            stroke_color,
        );

        // 2. 内部白色镂空（选中时）或底色
        let bg_color = if is_selected {
            self.theme.highlight_badge
        } else {
            self.theme.background
        };
        self.draw_rounded_rect(
            pixmap,
            RoundedRectParams {
                x: x + 1.0,
                y: y + 3.0,
                w: 10.0,
                h: 7.0,
                r: 1.0,
            },
            bg_color,
        );

        // 3. 顶部装订小竖线（左右两点）
        let mut pb = PathBuilder::new();
        pb.move_to(x + 3.0, y - 1.5);
        pb.line_to(x + 3.0, y + 1.5);
        pb.move_to(x + 9.0, y - 1.5);
        pb.line_to(x + 9.0, y + 1.5);

        if let Some(path) = pb.finish() {
            let stroke = Stroke {
                width: 1.0,
                ..Default::default()
            };
            let mut paint = Paint::default();
            paint.set_color(stroke_color.to_tiny_skia());
            paint.anti_alias = true;
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    /// 测量一段文字的排版宽度（优先从字形缓存中获取 advance_width，避免重复计算）
    fn measure_text(&self, text: &str, size: f32) -> f32 {
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

    /// 绘制指定坐标外层卡片背景（纯白微阴影与细描边）
    fn draw_card_background_at(
        &self,
        pixmap: &mut Pixmap,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) {
        let r = 10.0; // 圆角
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y);
        pb.line_to(x + width - r, y);
        pb.quad_to(x + width, y, x + width, y + r);
        pb.line_to(x + width, y + height - r);
        pb.quad_to(x + width, y + height, x + width - r, y + height);
        pb.line_to(x + r, y + height);
        pb.quad_to(x, y + height, x, y + height - r);
        pb.line_to(x, y + r);
        pb.quad_to(x, y, x + r, y);
        pb.close();

        if let Some(path) = pb.finish() {
            // 纯白填充
            let mut paint = Paint::default();
            paint.set_color(self.theme.background.to_tiny_skia());
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );

            // 1px 细描边
            let stroke = Stroke {
                width: 1.0,
                ..Default::default()
            };
            let mut border_paint = Paint::default();
            border_paint.set_color(self.theme.border.to_tiny_skia());
            border_paint.anti_alias = true;
            pixmap.stroke_path(&path, &border_paint, &stroke, Transform::identity(), None);
        }
    }

    /// 绘制小圆角矩形
    fn draw_rounded_rect(&self, pixmap: &mut Pixmap, params: RoundedRectParams, color: Color) {
        let mut pb = PathBuilder::new();
        pb.move_to(params.x + params.r, params.y);
        pb.line_to(params.x + params.w - params.r, params.y);
        pb.quad_to(
            params.x + params.w,
            params.y,
            params.x + params.w,
            params.y + params.r,
        );
        pb.line_to(params.x + params.w, params.y + params.h - params.r);
        pb.quad_to(
            params.x + params.w,
            params.y + params.h,
            params.x + params.w - params.r,
            params.y + params.h,
        );
        pb.line_to(params.x + params.r, params.y + params.h);
        pb.quad_to(
            params.x,
            params.y + params.h,
            params.x,
            params.y + params.h - params.r,
        );
        pb.line_to(params.x, params.y + params.r);
        pb.quad_to(params.x, params.y, params.x + params.r, params.y);
        pb.close();

        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color(color.to_tiny_skia());
            paint.anti_alias = true;
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// 绘制右侧下拉箭头
    fn draw_dropdown_arrow(&self, pixmap: &mut Pixmap, x: f32, y: f32) {
        let mut pb = PathBuilder::new();
        pb.move_to(x, y - 2.0);
        pb.line_to(x + 4.0, y + 2.0);
        pb.line_to(x + 8.0, y - 2.0);

        if let Some(path) = pb.finish() {
            let stroke = Stroke {
                width: 1.5,
                ..Default::default()
            };
            let mut paint = Paint::default();
            paint.set_color(self.theme.index_text.to_tiny_skia());
            paint.anti_alias = true;
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    /// 绘制文本
    fn draw_text(&self, pixmap: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, color: Color) {
        if self.font.is_none() {
            // 字体缺失降级：绘制字符占位小圆角线框（豆腐块），确保排版可见不白屏
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
                    r: 2.0,
                };
                self.draw_rounded_rect(pixmap, rect_params, placeholder_color);
                current_x += glyph_w + 3.0;
            }
            return;
        };

        let mut current_x = x;
        for ch in text.chars() {
            if let Some(glyph) = self.get_or_rasterize_glyph(ch, size) {
                if glyph.metrics.width > 0 && glyph.metrics.height > 0 {
                    let glyph_x = (current_x + glyph.metrics.xmin as f32) as i32;
                    let glyph_y =
                        (y - glyph.metrics.height as f32 - glyph.metrics.ymin as f32) as i32;

                    self.blend_glyph(
                        pixmap,
                        GlyphBitmapParams {
                            bitmap: &glyph.bitmap,
                            x: glyph_x,
                            y: glyph_y,
                            width: glyph.metrics.width,
                            height: glyph.metrics.height,
                        },
                        color,
                    );
                }
                current_x += glyph.metrics.advance_width;
            }
        }
    }

    /// 混合字形位图（采用标准 Porter-Duff Source-Over 预乘 Alpha 混合，消除文字抗锯齿黑边与底色镂空）
    fn blend_glyph(&self, pixmap: &mut Pixmap, params: GlyphBitmapParams, color: Color) {
        let pixmap_w = pixmap.width() as i32;
        let pixmap_h = pixmap.height() as i32;
        let pixels = pixmap.pixels_mut();

        for row in 0..params.height {
            let py = params.y + row as i32;
            if py < 0 || py >= pixmap_h {
                continue;
            }

            for col in 0..params.width {
                let px = params.x + col as i32;
                if px < 0 || px >= pixmap_w {
                    continue;
                }

                let alpha = params.bitmap[row * params.width + col] as f32 / 255.0;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::Candidate;

    #[test]
    fn test_render_horizontal_capsule() {
        let renderer = CandidateBoxRenderer::new(UiTheme::light());
        let state = CandidateBoxState {
            visible: true,
            preedit: "ri'qi".to_string(),
            candidates: vec![
                Candidate::new("日期", "ri'qi", 10_000),
                Candidate::new("日期", "ri'qi", 999_999)
                    .with_sub_candidates(vec!["2026年10月9日".to_string(), "20261009".to_string()]),
            ],
            selected_index: 0,
            is_expanded: false,
            expanded_selected_index: 0,
            mode_badge: "中".to_string(),
        };

        let pixmap = renderer.render(&state).expect("应成功渲染单行候选条");
        assert!(pixmap.width() > 100);
        assert_eq!(pixmap.height(), 38);
    }

    #[test]
    fn test_render_expanded_popup_card() {
        let renderer = CandidateBoxRenderer::new(UiTheme::light());
        let state = CandidateBoxState {
            visible: true,
            preedit: "ri'qi".to_string(),
            candidates: vec![
                Candidate::new("日期", "ri'qi", 10_000),
                Candidate::new("日期", "ri'qi", 999_999).with_sub_candidates(vec![
                    "2026年10月9日".to_string(),
                    "20261009".to_string(),
                    "2026-10-09".to_string(),
                    "2026/10/09".to_string(),
                    "2026.10.09".to_string(),
                    "二〇二六年十月九日".to_string(),
                    "丙午[马]年八月廿九".to_string(),
                ]),
            ],
            selected_index: 1,
            is_expanded: true,
            expanded_selected_index: 0,
            mode_badge: "中".to_string(),
        };

        let pixmap = renderer.render(&state).expect("应成功渲染展开的二级卡片");
        // 包含主单行条（38px）加间距（6px）加垂直卡片（7项*28px+12px = 208px）
        assert!(pixmap.height() > 200);
    }

    #[test]
    fn test_blend_glyph_source_over_preserves_opaque_background() {
        let renderer = CandidateBoxRenderer::new(UiTheme::light());
        let mut pixmap = Pixmap::new(4, 4).unwrap();
        // 填充纯白不透明底图 (R=255, G=255, B=255, A=255)
        pixmap.fill(tiny_skia::Color::WHITE);

        // 叠绘 50% 抗锯齿透明度的纯黑字形像素
        let bitmap = [128u8];
        renderer.blend_glyph(
            &mut pixmap,
            GlyphBitmapParams {
                bitmap: &bitmap,
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            },
            Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        );

        let p = pixmap.pixel(1, 1).unwrap();
        assert_eq!(
            p.alpha(),
            255,
            "在纯白不透明背景上混合半透明抗锯齿字形后，底图必须保持 100% 不透明"
        );
        assert!(
            p.red() > 100 && p.red() < 150,
            "混合后的灰度值应平滑处于中间区间，实际: {}",
            p.red()
        );
    }

    #[test]
    fn test_font_fallback_and_missing_font_render() {
        // 1. 验证默认字体检索路径包含多平台路径
        let search_paths = CandidateBoxRenderer::default_font_search_paths();
        assert!(search_paths.len() >= 10, "预设字体路径应覆盖主流桌面发行版");

        // 2. 模拟完全无字体的极端环境
        let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
        renderer.font = None;
        assert!(!renderer.has_font());

        let state = CandidateBoxState {
            visible: true,
            preedit: "ce'shi".to_string(),
            candidates: vec![
                Candidate::new("测试", "ce'shi", 100),
                Candidate::new("侧视", "ce'shi", 90),
            ],
            selected_index: 0,
            is_expanded: false,
            expanded_selected_index: 0,
            mode_badge: "中".to_string(),
        };

        // 必须成功光栅化，并利用豆腐块安全呈现，绝不返回 None 或触发 Panic
        let pixmap = renderer
            .render(&state)
            .expect("无字体极端环境下仍应成功渲染占位降级界面");
        assert!(pixmap.width() > 50);
        assert_eq!(pixmap.height(), 38);
    }

    #[test]
    fn test_glyph_cache_hit_and_reuse() {
        let renderer = CandidateBoxRenderer::new(UiTheme::light());
        if !renderer.has_font() {
            return;
        }

        renderer.clear_glyph_cache();
        assert_eq!(renderer.glyph_cache_len(), 0);

        let state = CandidateBoxState {
            visible: true,
            preedit: "ce".to_string(),
            candidates: vec![
                Candidate::new("测试", "ce'shi", 100),
                Candidate::new("侧视", "ce'shi", 90),
            ],
            selected_index: 0,
            is_expanded: false,
            expanded_selected_index: 0,
            mode_badge: "中".to_string(),
        };

        // 首次渲染：填充字形缓存
        let _ = renderer.render(&state);
        let cache_len_1 = renderer.glyph_cache_len();
        assert!(
            cache_len_1 > 0,
            "首次渲染后字形缓存中应包含已光栅化的字形位图"
        );

        // 二次渲染完全相同的候选词条：全部命中缓存，缓存数量不再增加，消除重复矢量运算
        let _ = renderer.render(&state);
        let cache_len_2 = renderer.glyph_cache_len();
        assert_eq!(
            cache_len_1, cache_len_2,
            "再次渲染相同字形应 100% 命中缓存，避免重复矢量光栅化"
        );
    }
}
