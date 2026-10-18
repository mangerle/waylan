use crate::theme::{Color, UiTheme};
use cosmic_text::{SwashContent, SwashImage};
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

/// 混合字形位图（支持 8-bit 单通道灰度 Mask 与 32-bit 彩色 Emoji，采用定点整数 Porter-Duff Source-Over 优化）
pub fn blend_swash_image(pixmap: &mut Pixmap, image: &SwashImage, pos: (i32, i32), color: Color) {
    let width = image.placement.width as i32;
    let height = image.placement.height as i32;
    if width <= 0 || height <= 0 {
        return;
    }

    let mut target = BlitTarget {
        pixmap_w: pixmap.width() as i32,
        pixmap_h: pixmap.height() as i32,
        pixels: pixmap.pixels_mut(),
    };

    match image.content {
        SwashContent::Mask | SwashContent::SubpixelMask => {
            blend_mask_glyph(&mut target, image, pos, color);
        }
        SwashContent::Color => {
            blend_color_emoji(&mut target, image, pos);
        }
    }
}

/// 绘图目标缓冲区上下文
struct BlitTarget<'a> {
    pixmap_w: i32,
    pixmap_h: i32,
    pixels: &'a mut [PremultipliedColorU8],
}

/// 混合单通道灰度字形遮罩
fn blend_mask_glyph(
    target: &mut BlitTarget<'_>,
    image: &SwashImage,
    pos: (i32, i32),
    color: Color,
) {
    let ca = (color.a * 255.0).round().clamp(0.0, 255.0) as u32;
    if ca == 0 {
        return;
    }
    let cr = (color.r * 255.0).round().clamp(0.0, 255.0) as u32;
    let cg = (color.g * 255.0).round().clamp(0.0, 255.0) as u32;
    let cb = (color.b * 255.0).round().clamp(0.0, 255.0) as u32;

    let (x, y) = pos;
    let width = image.placement.width as i32;
    let height = image.placement.height as i32;
    let bitmap = &image.data;

    for row in 0..height {
        let py = y + row;
        if py < 0 || py >= target.pixmap_h {
            continue;
        }
        let row_offset = (row * width) as usize;
        let py_offset = (py * target.pixmap_w) as usize;

        for col in 0..width {
            let px = x + col;
            if px < 0 || px >= target.pixmap_w {
                continue;
            }

            let cov = bitmap[row_offset + col as usize] as u32;
            if cov <= 2 {
                continue;
            }

            let src_a = (ca * cov + 127) / 255;
            if src_a == 0 {
                continue;
            }

            let src_r_premul = (cr * src_a + 127) / 255;
            let src_g_premul = (cg * src_a + 127) / 255;
            let src_b_premul = (cb * src_a + 127) / 255;

            let pixel = &mut target.pixels[py_offset + px as usize];
            blend_pixel_over(pixel, src_r_premul, src_g_premul, src_b_premul, src_a);
        }
    }
}

/// 混合 32-bit 彩色 Emoji 位图
fn blend_color_emoji(target: &mut BlitTarget<'_>, image: &SwashImage, pos: (i32, i32)) {
    let (x, y) = pos;
    let width = image.placement.width as i32;
    let height = image.placement.height as i32;
    let data = &image.data;

    for row in 0..height {
        let py = y + row;
        if py < 0 || py >= target.pixmap_h {
            continue;
        }
        let row_offset = (row * width * 4) as usize;
        let py_offset = (py * target.pixmap_w) as usize;

        for col in 0..width {
            let px = x + col;
            if px < 0 || px >= target.pixmap_w {
                continue;
            }

            let idx = row_offset + (col as usize * 4);
            if idx + 3 >= data.len() {
                continue;
            }
            let sa = data[idx + 3] as u32;
            if sa <= 2 {
                continue;
            }

            let sr = data[idx] as u32;
            let sg = data[idx + 1] as u32;
            let sb = data[idx + 2] as u32;

            let src_r_premul = (sr * sa + 127) / 255;
            let src_g_premul = (sg * sa + 127) / 255;
            let src_b_premul = (sb * sa + 127) / 255;

            let pixel = &mut target.pixels[py_offset + px as usize];
            blend_pixel_over(pixel, src_r_premul, src_g_premul, src_b_premul, sa);
        }
    }
}

/// 定点整数 Porter-Duff Source-Over 像素级原子混合
#[inline(always)]
fn blend_pixel_over(
    target: &mut PremultipliedColorU8,
    src_r_premul: u32,
    src_g_premul: u32,
    src_b_premul: u32,
    src_a: u32,
) {
    let inv_sa = 255 - src_a;
    let dst_a = target.alpha() as u32;
    let dst_r = target.red() as u32;
    let dst_g = target.green() as u32;
    let dst_b = target.blue() as u32;

    let out_a = (src_a + (dst_a * inv_sa + 127) / 255).min(255) as u8;
    let out_r = (src_r_premul + (dst_r * inv_sa + 127) / 255).min(out_a as u32) as u8;
    let out_g = (src_g_premul + (dst_g * inv_sa + 127) / 255).min(out_a as u32) as u8;
    let out_b = (src_b_premul + (dst_b * inv_sa + 127) / 255).min(out_a as u32) as u8;

    if let Some(blended) = PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a) {
        *target = blended;
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

/// 滚动条几何参数对象
#[derive(Debug, Clone, Copy)]
pub struct ScrollbarParams {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

/// 绘制细长胶囊滚动条指示器（采用参数对象收敛入参）
pub fn draw_scrollbar(pixmap: &mut Pixmap, params: ScrollbarParams, color: Color) {
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: params.x,
            y: params.y,
            w: params.width,
            h: params.height,
            r: params.radius,
        },
        color,
    );
}
