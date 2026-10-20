//! 字形位图与彩色 Emoji 的像素级 Alpha 混合光栅化
//!
//! # 设计原理
//! - **实现初衷**：候选框文本绘制需要把 cosmic-text 光栅化出的字形位图（8-bit 灰度遮罩或
//!   32-bit 彩色 Emoji）合成到 tiny-skia 像素缓冲区，该逐像素混合逻辑与矢量路径绘制
//!   完全无共享状态，混在同一文件会持续推高单文件行数并模糊关注点。
//! - **核心优势**：采用定点整数 Porter-Duff Source-Over，避开浮点运算与逐像素分支调用；
//!   与矢量绘制模块解耦后可独立演进与基准测试。
//! - **代价与局限**：逐像素双层循环为 CPU 密集型热路径，仅在字形尺寸较小的单帧文本合成下可接受。

use crate::theme::Color;
use cosmic_text::{SwashContent, SwashImage};
use tiny_skia::{Pixmap, PremultipliedColorU8};

/// 绘图目标缓冲区上下文
struct BlitTarget<'a> {
    pixmap_w: i32,
    pixmap_h: i32,
    pixels: &'a mut [PremultipliedColorU8],
}

/// 灰度字形遮罩的源色分量（预乘前的 8 位通道值）
#[derive(Clone, Copy)]
struct MaskSourceColor {
    r: u32,
    g: u32,
    b: u32,
    a: u32,
}

impl MaskSourceColor {
    /// 将浮点颜色量化为 8 位通道分量，颜色完全透明时返回 None
    fn from_color(color: Color) -> Option<Self> {
        let a = (color.a * 255.0).round().clamp(0.0, 255.0) as u32;
        if a == 0 {
            return None;
        }
        Some(Self {
            r: (color.r * 255.0).round().clamp(0.0, 255.0) as u32,
            g: (color.g * 255.0).round().clamp(0.0, 255.0) as u32,
            b: (color.b * 255.0).round().clamp(0.0, 255.0) as u32,
            a,
        })
    }
}

/// 像素级混合的源色参数
///
/// 承载预乘后的源色分量与源透明度，供 `blend_pixel_over` 在离屏光栅化热路径中
/// 以单次引用传递，避免平铺多个标量入参。
#[derive(Debug, Clone, Copy)]
struct PixelBlendParams {
    /// 预乘后的源红色分量
    r: u32,
    /// 预乘后的源绿色分量
    g: u32,
    /// 预乘后的源蓝色分量
    b: u32,
    /// 源透明度分量
    a: u32,
}

/// 混合字形位图（支持 8-bit 单通道灰度 Mask 与 32-bit 彩色 Emoji，采用定点整数 Porter-Duff Source-Over 优化）
pub(crate) fn blend_swash_image(
    pixmap: &mut Pixmap,
    image: &SwashImage,
    pos: (i32, i32),
    color: Color,
) {
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

/// 计算灰度覆盖率对应的预乘源色分量（覆盖率过低导致完全透明时返回 None）
fn premultiply_mask_color(source: MaskSourceColor, cov: u32) -> Option<(u32, u32, u32, u32)> {
    let src_a = (source.a * cov + 127) / 255;
    if src_a == 0 {
        return None;
    }
    let src_r_premul = (source.r * src_a + 127) / 255;
    let src_g_premul = (source.g * src_a + 127) / 255;
    let src_b_premul = (source.b * src_a + 127) / 255;
    Some((src_r_premul, src_g_premul, src_b_premul, src_a))
}

/// 混合单通道灰度字形遮罩
fn blend_mask_glyph(
    target: &mut BlitTarget<'_>,
    image: &SwashImage,
    pos: (i32, i32),
    color: Color,
) {
    let Some(source) = MaskSourceColor::from_color(color) else {
        return;
    };

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

            if let Some((src_r, src_g, src_b, src_a)) = premultiply_mask_color(source, cov) {
                let pixel = &mut target.pixels[py_offset + px as usize];
                blend_pixel_over(
                    pixel,
                    &PixelBlendParams {
                        r: src_r,
                        g: src_g,
                        b: src_b,
                        a: src_a,
                    },
                );
            }
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
            blend_pixel_over(
                pixel,
                &PixelBlendParams {
                    r: src_r_premul,
                    g: src_g_premul,
                    b: src_b_premul,
                    a: sa,
                },
            );
        }
    }
}

/// 定点整数 Porter-Duff Source-Over 像素级原子混合
#[inline(always)]
fn blend_pixel_over(target: &mut PremultipliedColorU8, src: &PixelBlendParams) {
    let src_a = src.a;
    let inv_sa = 255 - src_a;
    let dst_a = target.alpha() as u32;
    let dst_r = target.red() as u32;
    let dst_g = target.green() as u32;
    let dst_b = target.blue() as u32;

    let out_a = (src_a + (dst_a * inv_sa + 127) / 255).min(255) as u8;
    let out_r = (src.r + (dst_r * inv_sa + 127) / 255).min(out_a as u32) as u8;
    let out_g = (src.g + (dst_g * inv_sa + 127) / 255).min(out_a as u32) as u8;
    let out_b = (src.b + (dst_b * inv_sa + 127) / 255).min(out_a as u32) as u8;

    if let Some(blended) = PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a) {
        *target = blended;
    }
}
