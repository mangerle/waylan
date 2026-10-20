//! 候选框几何布局度量模块
//!
//! # 设计原理
//! - **实现初衷**：将候选框的尺寸计算（排版度量、浮层定位、画布规格）从渲染器主循环中剥离为独立关注点。
//! - **核心优势**：渲染器主结构仅保留文本引擎与视图分派，几何计算可独立演进与复用。
//! - **代价与局限**：几何度量依赖渲染器的字体测量能力，仍需以 `&mut` 借用渲染器实例。

use super::layout::{measure_candidates_bar, ItemMeasure, LayoutParams};
use super::CandidateBoxRenderer;
use waylan_core::CandidateBoxState;

/// 胶囊候选栏与展开浮层卡片几何布局度量结果
pub(crate) struct CandidateBoxGeometry<'a> {
    pub(crate) measured_items: Vec<ItemMeasure>,
    pub(crate) total_bar_width: f32,
    pub(crate) bar_height: f32,
    pub(crate) padding_h: f32,
    pub(crate) corner_radius: f32,
    pub(crate) font_size: f32,
    pub(crate) scale: f32,
    pub(crate) has_active_sub: bool,
    pub(crate) sub_candidates: &'a [String],
    pub(crate) popup_x: f32,
    pub(crate) popup_w: f32,
    pub(crate) popup_h: f32,
    pub(crate) row_height: f32,
    pub(crate) canvas_w: u32,
    pub(crate) canvas_h: u32,
}

/// 候选框基础排版度量参数（由渲染器样式与缩放因子统一派生）
struct CandidateBoxBaseMetrics {
    scale: f32,
    bar_height: f32,
    padding_h: f32,
    corner_radius: f32,
    item_gap: f32,
    font_size: f32,
}

/// 候选框原始画布尺寸计算参数
struct CandidateCanvasSizeParams {
    has_active_sub: bool,
    total_bar_width: f32,
    popup_x: f32,
    popup_w: f32,
    popup_h: f32,
    bar_height: f32,
    padding_h: f32,
    scale: f32,
}

/// 展开浮层几何度量输入参数
///
/// # 设计原理
/// - **实现初衷**：浮层尺寸依赖渲染样式派生的缩放、字号、内边距与候选项实测宽度，
///   四项平铺传参易与调用点的顺序错配。
/// - **核心优势**：以参数对象聚合同源度量输入，保证与候选条实测结果口径一致。
/// - **代价与局限**：调用点需显式书写字段名。
struct PopupMeasureParams<'a> {
    /// 界面缩放因子
    s: f32,
    /// 候选词字号
    font_size: f32,
    /// 候选条水平内边距
    padding_h: f32,
    /// 已完成排版测量的候选项尺寸列表
    measured_items: &'a [ItemMeasure],
}

impl CandidateBoxRenderer {
    /// 依据渲染器样式解析候选框的基础排版度量参数
    fn resolve_base_metrics(&self) -> CandidateBoxBaseMetrics {
        let s = self.scale.max(0.5);
        CandidateBoxBaseMetrics {
            scale: s,
            bar_height: self.style.bar_height * s,
            padding_h: self.style.padding_h.max(4.0) * s,
            corner_radius: self.style.corner_radius.max(2.0) * s,
            item_gap: self.style.item_gap.max(2.0) * s,
            font_size: self.font_size * s,
        }
    }

    /// 测量二级展开子候选文本的最大排版宽度
    fn measure_max_sub_candidate_width(
        &mut self,
        sub_candidates: &[String],
        font_size: f32,
    ) -> f32 {
        sub_candidates
            .iter()
            .map(|item| self.measure_text(item, font_size))
            .fold(0.0_f32, f32::max)
    }

    /// 计算胶囊候选栏与展开浮层的几何尺寸与画布规格
    /// 度量二级展开卡片几何尺寸与相对位置
    fn measure_popup_dimensions<'a>(
        &mut self,
        state: &'a CandidateBoxState,
        params: &PopupMeasureParams<'_>,
    ) -> (bool, &'a [String], f32, f32, f32, f32) {
        let s = params.s;
        let font_size = params.font_size;
        let padding_h = params.padding_h;
        let measured_items = params.measured_items;
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

        let max_sub_w = self.measure_max_sub_candidate_width(sub_candidates, font_size);
        let popup_w: f32 = (max_sub_w + 48.0 * s).max(190.0 * s);

        let active_item_x = measured_items
            .get(state.selected_index)
            .map(|m| m.start_x)
            .unwrap_or(padding_h);
        let popup_x = active_item_x.max(padding_h);

        (
            has_active_sub,
            sub_candidates,
            popup_x,
            popup_w,
            popup_h,
            row_height,
        )
    }

    /// 校验原始宽高边界并计算对齐后的画布物理像素规格
    fn calculate_aligned_canvas_size(raw_w: f32, raw_h: f32, s: f32) -> Option<(u32, u32)> {
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
            None
        } else {
            Some((canvas_w, canvas_h))
        }
    }

    /// 测量单行胶囊候选条中各候选项的排版尺寸
    fn measure_capsule_items(
        &mut self,
        state: &CandidateBoxState,
        metrics: &CandidateBoxBaseMetrics,
    ) -> (Vec<ItemMeasure>, f32) {
        measure_candidates_bar(
            LayoutParams {
                candidates: &state.candidates,
                selected_index: state.selected_index,
                preedit: &state.preedit,
                mode_badge: &state.mode_badge,
                index_style: self.style.index_style,
                show_comment: self.style.show_pinyin_comment,
                s: metrics.scale,
                padding_h: metrics.padding_h,
                item_gap: metrics.item_gap,
                font_size: metrics.font_size,
            },
            |text, size| self.measure_text(text, size),
        )
    }

    /// 计算候选框在浮层展开与收起两种形态下的原始画布尺寸并对齐为物理像素规格
    fn calculate_canvas_size(&self, params: &CandidateCanvasSizeParams) -> Option<(u32, u32)> {
        let s = params.scale;
        let raw_w = if params.has_active_sub {
            params
                .total_bar_width
                .max(params.popup_x + params.popup_w + params.padding_h)
        } else {
            params.total_bar_width
        }
        .ceil();
        let raw_h = if params.has_active_sub {
            params.bar_height + 6.0 * s + params.popup_h
        } else {
            params.bar_height
        }
        .ceil();

        Self::calculate_aligned_canvas_size(raw_w, raw_h, s)
    }

    /// 计算胶囊候选栏与展开浮层的几何尺寸与画布规格
    pub(crate) fn calculate_geometry<'a>(
        &mut self,
        state: &'a CandidateBoxState,
    ) -> Option<CandidateBoxGeometry<'a>> {
        let metrics = self.resolve_base_metrics();

        let (measured_items, total_bar_width) = self.measure_capsule_items(state, &metrics);

        let (has_active_sub, sub_candidates, popup_x, popup_w, popup_h, row_height) = self
            .measure_popup_dimensions(
                state,
                &PopupMeasureParams {
                    s: metrics.scale,
                    font_size: metrics.font_size,
                    padding_h: metrics.padding_h,
                    measured_items: &measured_items,
                },
            );

        let (canvas_w, canvas_h) = self.calculate_canvas_size(&CandidateCanvasSizeParams {
            has_active_sub,
            total_bar_width,
            popup_x,
            popup_w,
            popup_h,
            bar_height: metrics.bar_height,
            padding_h: metrics.padding_h,
            scale: metrics.scale,
        })?;

        Some(CandidateBoxGeometry {
            measured_items,
            total_bar_width,
            bar_height: metrics.bar_height,
            padding_h: metrics.padding_h,
            corner_radius: metrics.corner_radius,
            font_size: metrics.font_size,
            scale: metrics.scale,
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
}
