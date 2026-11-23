use std::borrow::Cow;

use waylan_core::{Candidate, CandidateIndexStyle, format_candidate_index};

/// 单个候选条测量计算结果
pub struct ItemMeasure {
    pub index_str: String,
    pub index_width: f32,
    pub text_width: f32,
    pub is_expandable: bool,
    pub icon_width: f32,
    pub total_item_width: f32,
    pub start_x: f32,
}

/// 布局计算输入参数
pub struct LayoutParams<'a> {
    pub candidates: &'a [Candidate],
    pub selected_index: usize,
    pub preedit: &'a str,
    pub mode_badge: &'a str,
    pub index_style: CandidateIndexStyle,
    pub show_comment: bool,
    pub s: f32,
    pub padding_h: f32,
    pub item_gap: f32,
    pub font_size: f32,
}

/// 单个候选条目的测量参数
struct CandidateItemMeasureParams<'a> {
    cand: &'a Candidate,
    index: usize,
    index_style: CandidateIndexStyle,
    show_comment: bool,
    is_selected: bool,
    font_size: f32,
    scale: f32,
    start_x: f32,
}

/// 计算整行候选条的测量结果与尺寸
pub fn measure_candidates_bar<'a, F>(
    params: LayoutParams<'a>,
    mut measure_fn: F,
) -> (Vec<ItemMeasure>, f32)
where
    F: FnMut(&str, f32) -> f32,
{
    let mut measured = Vec::with_capacity(params.candidates.len());

    let (badge_offset, preedit_offset) = measure_candidate_prefix_offsets(&params, &mut measure_fn);
    let mut current_pos_x = params.padding_h + 2.0 * params.s;
    current_pos_x += badge_offset;
    current_pos_x += preedit_offset;

    for (idx, cand) in params.candidates.iter().enumerate() {
        let item = measure_candidate_item(
            &CandidateItemMeasureParams {
                cand,
                index: idx,
                index_style: params.index_style,
                show_comment: params.show_comment,
                is_selected: idx == params.selected_index,
                font_size: params.font_size,
                scale: params.s,
                start_x: current_pos_x,
            },
            &mut measure_fn,
        );
        let item_width = item.total_item_width;
        measured.push(item);
        current_pos_x += item_width + params.item_gap;
    }

    let action_area_width = 24.0 * params.s;
    let total_bar_width = current_pos_x - params.item_gap + action_area_width + params.padding_h;
    (measured, total_bar_width)
}

/// 测量模式标识徽标与预编辑拼音各自的前置占位宽度（含各自后置间距）
fn measure_candidate_prefix_offsets<F>(params: &LayoutParams<'_>, measure_fn: &mut F) -> (f32, f32)
where
    F: FnMut(&str, f32) -> f32,
{
    // 若配置了模式标识（如“中”、“英”、“V”），前置预留徽标宽度
    let badge_offset = if params.mode_badge.is_empty() {
        0.0
    } else {
        let badge_w = measure_fn(params.mode_badge, params.font_size * 0.8) + 10.0 * params.s;
        badge_w + 6.0 * params.s
    };

    // 若存在预编辑拼音，前置预留预编辑文本区域
    let preedit_offset = if params.preedit.is_empty() {
        0.0
    } else {
        let preedit_w = measure_fn(params.preedit, params.font_size * 0.9) + 8.0 * params.s;
        preedit_w + 8.0 * params.s
    };

    (badge_offset, preedit_offset)
}

/// 根据展示开关拼接候选词与注释文本（无注释或关闭展示时零堆分配借用原切片）
#[inline]
pub(crate) fn format_candidate_text<'a>(cand: &'a Candidate, show_comment: bool) -> Cow<'a, str> {
    if show_comment {
        match cand.comment.as_deref() {
            Some(comment) if !comment.is_empty() => {
                Cow::Owned(format!("{} {}", cand.text, comment))
            }
            _ => Cow::Borrowed(cand.text.as_str()),
        }
    } else {
        Cow::Borrowed(cand.text.as_str())
    }
}

/// 测量单个候选条目的序号、注释、图标与文本宽度明细
fn measure_candidate_item<F>(
    params: &CandidateItemMeasureParams<'_>,
    measure_fn: &mut F,
) -> ItemMeasure
where
    F: FnMut(&str, f32) -> f32,
{
    let s = params.scale;
    let index_str = format_candidate_index(params.index_style, params.index);
    let index_width = if index_str.is_empty() {
        0.0
    } else {
        measure_fn(&index_str, params.font_size * 0.85)
    };
    let display_text = format_candidate_text(params.cand, params.show_comment);
    let text_width = measure_fn(&display_text, params.font_size);
    let icon_width = if params.cand.is_expandable {
        16.0 * s
    } else {
        0.0
    };

    let item_width = if params.is_selected {
        index_width + icon_width + text_width + 16.0 * s
    } else {
        index_width + 4.0 * s + icon_width + text_width
    };

    ItemMeasure {
        index_str,
        index_width,
        text_width,
        is_expandable: params.cand.is_expandable,
        icon_width,
        total_item_width: item_width,
        start_x: params.start_x,
    }
}

/// 鼠标在候选框上的命中交互目标
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateHitTarget {
    /// 选中指定候选词（包含单行展示页索引、二级展开卡片子项索引或多行网格全局索引）
    Select(usize),
    /// 展开或折叠 5 行大网格候选面板
    ToggleExpand,
    /// 翻上一页
    PrevPage,
    /// 翻下一页
    NextPage,
}

/// 鼠标点击检测矩形区域
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub target: CandidateHitTarget,
}

/// 命中检测区域的构造参数
///
/// # 设计原理
/// - **实现初衷**：命中区域由"矩形边界 + 交互目标"两类异质信息合成，
///   平铺为五个入参会让调用点的实参与形参一一对应，易发生顺序错位。
/// - **核心优势**：以参数对象聚合语义字段，调用点按名传参即可完成构造，杜绝位置传参歧义。
/// - **代价与局限**：多引入一个结构体类型，调用点需显式书写字段名。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitRegionParams {
    /// 区域左上角横坐标
    pub x: f32,
    /// 区域左上角纵坐标
    pub y: f32,
    /// 区域宽度
    pub width: f32,
    /// 区域高度
    pub height: f32,
    /// 命中该区域后触发的交互动作
    pub target: CandidateHitTarget,
}

impl HitRegion {
    /// 依据命中检测区域参数构造命中区域
    pub fn new(params: HitRegionParams) -> Self {
        Self {
            x: params.x,
            y: params.y,
            width: params.width,
            height: params.height,
            target: params.target,
        }
    }

    /// 判定给定点是否落在本命中矩形区域内（含边界）
    #[inline]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }
}
