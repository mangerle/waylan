//! 候选框渲染样式参数对象
//!
//! 将用户配置中的密度、间距、内边距、圆角、主题与序号风格收敛为不可变渲染参数，
//! 避免绘制代码散落魔法数字，并保证 Wayland 自绘候选框与 GNOME 扩展的视觉配置语义一致。

use waylan_core::config::{CandidateDensity, CandidateIndexStyle, CandidateThemePreset, UiConfig};

/// 候选框渲染样式参数
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RendererStyle {
    /// 单行候选条高度（逻辑像素）
    pub bar_height: f32,
    /// 候选词项间距（逻辑像素）
    pub item_gap: f32,
    /// 左右内边距（逻辑像素）
    pub padding_h: f32,
    /// 圆角半径（逻辑像素）
    pub corner_radius: f32,
    /// 背景不透明度百分比（70 ~ 100）
    pub opacity: u32,
    /// 候选序号展示风格
    pub index_style: CandidateIndexStyle,
    /// 是否显示候选词注释
    pub show_pinyin_comment: bool,
    /// 候选框色彩主题预设
    pub theme_preset: CandidateThemePreset,
}

impl Default for RendererStyle {
    fn default() -> Self {
        Self {
            bar_height: 38.0,
            item_gap: 14.0,
            padding_h: 10.0,
            corner_radius: 10.0,
            opacity: 100,
            index_style: CandidateIndexStyle::default(),
            show_pinyin_comment: false,
            theme_preset: CandidateThemePreset::default(),
        }
    }
}

impl From<&UiConfig> for RendererStyle {
    fn from(ui: &UiConfig) -> Self {
        let density_scale: f32 = match ui.candidate_density {
            CandidateDensity::Compact => 0.9,
            CandidateDensity::Normal => 1.0,
            CandidateDensity::Spacious => 1.1,
        };
        // 上下内边距直接参与单行高度，保持与用户设置中的排版参数一致
        let bar_height = (38.0 * density_scale + (ui.candidate_padding_v - 4.0)).max(28.0);
        Self {
            bar_height,
            item_gap: ui.candidate_item_spacing,
            padding_h: ui.candidate_padding_h,
            corner_radius: ui.candidate_corner_radius,
            opacity: ui.candidate_opacity,
            index_style: ui.candidate_index_style,
            show_pinyin_comment: ui.show_pinyin_comment,
            theme_preset: ui.candidate_theme,
        }
    }
}
