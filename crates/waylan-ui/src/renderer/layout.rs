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
    pub s: f32,
    pub padding_h: f32,
    pub item_gap: f32,
    pub font_size: f32,
}

/// 计算整行候选条的测量结果与尺寸
pub fn measure_candidates_bar<'a, F>(
    params: LayoutParams<'a>,
    measure_fn: F,
) -> (Vec<ItemMeasure<'a>>, f32)
where
    F: Fn(&str, f32) -> f32,
{
    let mut measured = Vec::with_capacity(params.candidates.len());
    let mut current_pos_x = params.padding_h + 2.0 * params.s;

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
