use waylan_core::Candidate;

/// 常用候选序号预缓存字符串，杜绝逐帧 format 堆分配
pub const PRECACHED_INDEX_STRINGS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];

/// 单个候选条测量计算结果
pub struct ItemMeasure<'a> {
    pub index_str: &'a str,
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
    pub s: f32,
    pub padding_h: f32,
    pub item_gap: f32,
    pub font_size: f32,
}

/// 计算整行候选条的测量结果与尺寸
pub fn measure_candidates_bar<'a, F>(
    params: LayoutParams<'a>,
    mut measure_fn: F,
) -> (Vec<ItemMeasure<'a>>, f32)
where
    F: FnMut(&str, f32) -> f32,
{
    let mut measured = Vec::with_capacity(params.candidates.len());
    let mut current_pos_x = params.padding_h + 2.0 * params.s;

    // 若配置了模式标识（如“中”、“英”、“V”），前置预留徽标宽度
    if !params.mode_badge.is_empty() {
        let badge_w = measure_fn(params.mode_badge, params.font_size * 0.8) + 10.0 * params.s;
        current_pos_x += badge_w + 6.0 * params.s;
    }

    // 若存在预编辑拼音，前置预留预编辑文本区域
    if !params.preedit.is_empty() {
        let preedit_w = measure_fn(params.preedit, params.font_size * 0.9) + 8.0 * params.s;
        current_pos_x += preedit_w + 8.0 * params.s;
    }

    for (idx, cand) in params.candidates.iter().enumerate() {
        let index_str = if idx < PRECACHED_INDEX_STRINGS.len() {
            PRECACHED_INDEX_STRINGS[idx]
        } else {
            "•"
        };

        let index_width = measure_fn(index_str, params.font_size * 0.85);
        let text_width = measure_fn(&cand.text, params.font_size);
        let icon_width = if cand.is_expandable {
            16.0 * params.s
        } else {
            0.0
        };

        let is_selected = idx == params.selected_index;
        let item_width = if is_selected {
            index_width + icon_width + text_width + 16.0 * params.s
        } else {
            index_width + 4.0 * params.s + icon_width + text_width
        };

        measured.push(ItemMeasure {
            index_str,
            index_width,
            text_width,
            is_expandable: cand.is_expandable,
            icon_width,
            total_item_width: item_width,
            start_x: current_pos_x,
        });

        current_pos_x += item_width + params.item_gap;
    }

    let action_area_width = 24.0 * params.s;
    let total_bar_width = current_pos_x - params.item_gap + action_area_width + params.padding_h;
    (measured, total_bar_width)
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

impl HitRegion {
    pub fn new(x: f32, y: f32, width: f32, height: f32, target: CandidateHitTarget) -> Self {
        Self {
            x,
            y,
            width,
            height,
            target,
        }
    }

    #[inline]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }
}
