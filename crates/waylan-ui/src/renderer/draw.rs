//! 候选框矢量路径绘制原语（圆角矩形、卡片背景、箭头与图标）
//!
//! # 设计原理
//! - **实现初衷**：将基于 tiny-skia 路径构建的矢量图形绘制从渲染器主循环中剥离为可复用的原子绘图函数。
//! - **核心优势**：所有绘图入口统一采用参数对象收敛几何与配色入参，绘制坐标运算集中且可独立验证。
//! - **代价与局限**：字形位图的逐像素合成不属于矢量绘制，已拆分至 `glyph_blend` 独立模块。

use crate::theme::{Color, UiTheme};
use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

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
    if let Some(path) = build_card_background_path(&params) {
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

/// 构建圆角卡片背景的矢量路径（依次描出上、右、下、左四条边与四角圆弧）
fn build_card_background_path(params: &CardBackgroundParams) -> Option<Path> {
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
    pb.finish()
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

/// 下拉小箭头绘制参数
///
/// 承载箭头尖端的锚点坐标与缩放系数，用于 `draw_dropdown_arrow` 以单次引用
/// 传递几何参数，避免平铺多个标量入参。
#[derive(Debug, Clone, Copy)]
pub struct DropdownArrowParams {
    /// 箭头尖端的横坐标
    pub x: f32,
    /// 箭头尖端的纵坐标
    pub y: f32,
    /// 箭头缩放系数（决定箭头整体尺寸与线宽）
    pub scale: f32,
}

/// 绘制右侧下拉小箭头
pub fn draw_dropdown_arrow(pixmap: &mut Pixmap, theme: &UiTheme, params: &DropdownArrowParams) {
    let x = params.x;
    let y = params.y;
    let s = params.scale;
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

/// 日历图标绘制参数
///
/// 承载微型日历图标的位置、选中态与缩放系数，供 `draw_calendar_icon` 及其
/// 子绘制步骤共享同一份几何上下文，避免平铺多个标量入参。
#[derive(Debug, Clone, Copy)]
pub struct CalendarIconParams {
    /// 图标左上角的横坐标
    pub x: f32,
    /// 图标左上角的纵坐标
    pub y: f32,
    /// 是否处于选中态（决定描边色与内芯底色）
    pub is_selected: bool,
    /// 图标缩放系数
    pub scale: f32,
}

/// 绘制微型矢量日历图标（针对可展开功能项）
pub fn draw_calendar_icon(pixmap: &mut Pixmap, theme: &UiTheme, params: &CalendarIconParams) {
    let stroke_color = if params.is_selected {
        theme.highlight_text
    } else {
        theme.index_text
    };

    draw_calendar_icon_body(pixmap, theme, params, stroke_color);
    draw_calendar_icon_binders(pixmap, params, stroke_color);
}

/// 绘制日历图标的外框描边色块与内芯底色
fn draw_calendar_icon_body(
    pixmap: &mut Pixmap,
    theme: &UiTheme,
    params: &CalendarIconParams,
    stroke_color: Color,
) {
    let s = params.scale;
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: params.x,
            y: params.y,
            w: 12.0 * s,
            h: 11.0 * s,
            r: 2.0 * s,
        },
        stroke_color,
    );

    let bg_color = if params.is_selected {
        theme.highlight_badge
    } else {
        theme.background
    };
    draw_rounded_rect(
        pixmap,
        RoundedRectParams {
            x: params.x + 1.0 * s,
            y: params.y + 3.0 * s,
            w: 10.0 * s,
            h: 7.0 * s,
            r: 1.0 * s,
        },
        bg_color,
    );
}

/// 绘制日历图标顶部的两条装订竖线
fn draw_calendar_icon_binders(
    pixmap: &mut Pixmap,
    params: &CalendarIconParams,
    stroke_color: Color,
) {
    let s = params.scale;
    let x = params.x;
    let y = params.y;
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

/// 向上折叠箭头绘制参数
///
/// 承载箭头中心坐标、尺寸与填充色，供 `draw_collapse_arrow` 以单次引用传递
/// 几何与配色参数，避免平铺多个标量入参。
#[derive(Debug, Clone, Copy)]
pub struct CollapseArrowParams {
    /// 箭头中心的横坐标
    pub center_x: f32,
    /// 箭头中心的纵坐标
    pub center_y: f32,
    /// 箭头边长尺寸
    pub size: f32,
    /// 箭头填充颜色
    pub color: Color,
}

/// 绘制向上折叠箭头（点击收回单行模式）
pub fn draw_collapse_arrow(pixmap: &mut Pixmap, params: &CollapseArrowParams) {
    let center_x = params.center_x;
    let center_y = params.center_y;
    let size = params.size;
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
        paint.set_color(params.color.to_tiny_skia());
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
