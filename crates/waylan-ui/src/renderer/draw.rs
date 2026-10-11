use crate::renderer::font::CachedGlyph;
use crate::theme::{Color, UiTheme};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform};

/// 圆角矩形几何参数
pub struct RoundedRectParams {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: f32,
}

/// 卡片背景几何与描边参数
pub struct CardBackgroundParams {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub stroke_width: f32,
}

/// 绘制外层卡片背景（纯白微阴影与细描边）
pub fn draw_card_background_at(pixmap: &mut Pixmap, theme: &UiTheme, params: CardBackgroundParams) {
    let r = params.radius;
    let mut pb = PathBuilder::new();
    pb.move_to(params.x + r, params.y);
    pb.line_to(params.x + params.width - r, params.y);
    pb.quad_to(
        params.x + params.width,
        params.y,
        params.x + params.width,
        params.y + r,
    );
    pb.line_to(params.x + params.width, params.y + params.height - r);
    pb.quad_to(
        params.x + params.width,
        params.y + params.height,
        params.x + params.width - r,
        params.y + params.height,
    );
    pb.line_to(params.x + r, params.y + params.height);
    pb.quad_to(
        params.x,
        params.y + params.height,
        params.x,
        params.y + params.height - r,
    );
    pb.line_to(params.x, params.y + r);
    pb.quad_to(params.x, params.y, params.x + r, params.y);
    pb.close();

    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(theme.background.to_tiny_skia());
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );

        let stroke = Stroke {
            width: params.stroke_width,
            ..Default::default()
        };
        let mut border_paint = Paint::default();
        border_paint.set_color(theme.border.to_tiny_skia());
        border_paint.anti_alias = true;
        pixmap.stroke_path(&path, &border_paint, &stroke, Transform::identity(), None);
    }
}

/// 绘制圆角矩形填充
pub fn draw_rounded_rect(pixmap: &mut Pixmap, params: RoundedRectParams, color: Color) {
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

/// 绘制右侧下拉小箭头
pub fn draw_dropdown_arrow(pixmap: &mut Pixmap, theme: &UiTheme, x: f32, y: f32, s: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y - 2.0 * s);
    pb.line_to(x + 4.0 * s, y + 2.0 * s);
    pb.line_to(x + 8.0 * s, y - 2.0 * s);

    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width: 1.5 * s,
            ..Default::default()
        };
        let mut paint = Paint::default();
        paint.set_color(theme.index_text.to_tiny_skia());
        paint.anti_alias = true;
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

/// 绘制微型矢量日历图标（针对可展开功能项）
pub fn draw_calendar_icon(
    pixmap: &mut Pixmap,
    theme: &UiTheme,
    x: f32,
    y: f32,
    is_selected: bool,
    s: f32,
) {
    let stroke_color = if is_selected {
        theme.highlight_text
    } else {
        theme.index_text
    };

    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x,
            y,
            w: 12.0 * s,
            h: 11.0 * s,
            r: 2.0 * s,
        },
        stroke_color,
    );

    let bg_color = if is_selected {
        theme.highlight_badge
    } else {
        theme.background
    };
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: x + 1.0 * s,
            y: y + 3.0 * s,
            w: 10.0 * s,
            h: 7.0 * s,
            r: 1.0 * s,
        },
        bg_color,
    );

    let mut pb = PathBuilder::new();
    pb.move_to(x + 3.0 * s, y - 1.5 * s);
    pb.line_to(x + 3.0 * s, y + 1.5 * s);
    pb.move_to(x + 9.0 * s, y - 1.5 * s);
    pb.line_to(x + 9.0 * s, y + 1.5 * s);

    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width: 1.0 * s,
            ..Default::default()
        };
        let mut paint = Paint::default();
        paint.set_color(stroke_color.to_tiny_skia());
        paint.anti_alias = true;
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

/// 混合字形位图（采用标准 Porter-Duff Source-Over）
pub fn blend_glyph(pixmap: &mut Pixmap, glyph: &CachedGlyph, pos: (i32, i32), color: Color) {
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

            if let Some(blended) = PremultipliedColorU8::from_rgba(
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

/// 绘制向上折叠箭头（点击收回单行模式）
pub fn draw_collapse_arrow(
    pixmap: &mut Pixmap,
    center_x: f32,
    center_y: f32,
    size: f32,
    color: Color,
) {
    let half_w = size * 0.5;
    let half_h = size * 0.35;
    let mut pb = PathBuilder::new();
    // 向上三角形
    pb.move_to(center_x, center_y - half_h);
    pb.line_to(center_x + half_w, center_y + half_h);
    pb.line_to(center_x - half_w, center_y + half_h);
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

/// 绘制细长胶囊滚动条指示器
pub fn draw_scrollbar(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    color: Color,
) {
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x,
            y,
            w: width,
            h: height,
            r: radius,
        },
        color,
    );
}
