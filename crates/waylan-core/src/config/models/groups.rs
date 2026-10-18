//! 分组配置数据结构体定义（界面视觉、快捷键、拼音算法、桌面托盘）
//!
//! # 设计原理
//! - **实现初衷**：消除 `AppConfig` 平铺近 50 个字段的坏味道，按照业务领域划分为高内聚的子配置结构体。
//! - **核心优势**：职责清晰、类型完备，支持按子领域独立测试与序列化；结合 `#[serde(flatten)]` 实现向前完全兼容。
//! - **代价与局限**：调用时需通过子模块命名空间访问（如 `cfg.ui.font_size`）。

use serde::{Deserialize, Serialize};

use super::general::{
    CandidateDensity, CandidateIndexStyle, CandidateOrientation, CandidateThemePreset,
    CandidateWindowMode, FuzzyPinyinRules, PageKeys, SwitchKey, ThemeMode, TypoCorrectionRules,
};

fn default_candidate_item_spacing() -> f32 {
    8.0
}

fn default_candidate_padding_v() -> f32 {
    4.0
}

fn default_candidate_padding_h() -> f32 {
    8.0
}

fn default_candidate_corner_radius() -> f32 {
    8.0
}

fn default_candidate_opacity() -> u32 {
    95
}

/// 界面视觉排版与候选框渲染配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    /// 单页候选词数量（默认 5）
    pub page_size: usize,
    /// 候选文字大小（像素）
    pub font_size: f32,
    /// 界面主题模式
    pub theme: ThemeMode,
    /// 候选词排列方向（水平 / 垂直）
    pub candidate_orientation: CandidateOrientation,
    /// 候选词序号展示风格（默认现代空白风）
    pub candidate_index_style: CandidateIndexStyle,
    /// 候选框渲染模式（微澜现代专属 / 桌面原生系统）
    pub candidate_window_mode: CandidateWindowMode,
    /// 候选框排版留白密度（紧凑 / 标准 / 宽松）
    pub candidate_density: CandidateDensity,
    /// 候选框色彩主题预设（微澜翡翠绿 / 极简黑白 / 极客暗夜冰蓝 / 暖阳奶茶）
    pub candidate_theme: CandidateThemePreset,
    /// 候选词项间距（像素，范围 2.0 ~ 24.0，默认 8.0）
    #[serde(default = "default_candidate_item_spacing")]
    pub candidate_item_spacing: f32,
    /// 候选框上下内边距（像素，范围 2.0 ~ 16.0，默认 4.0）
    #[serde(default = "default_candidate_padding_v")]
    pub candidate_padding_v: f32,
    /// 候选框左右内边距（像素，范围 4.0 ~ 24.0，默认 8.0）
    #[serde(default = "default_candidate_padding_h")]
    pub candidate_padding_h: f32,
    /// 候选框圆角曲率半径（像素，范围 0.0 ~ 20.0，默认 8.0）
    #[serde(default = "default_candidate_corner_radius")]
    pub candidate_corner_radius: f32,
    /// 候选框背景不透明度百分比（范围 70 ~ 100，默认 95）
    #[serde(default = "default_candidate_opacity")]
    pub candidate_opacity: u32,
    /// 是否在候选词旁边附加拼音/注释提示
    pub show_pinyin_comment: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            page_size: 5,
            font_size: 15.0,
            theme: ThemeMode::Light,
            candidate_orientation: CandidateOrientation::Horizontal,
            candidate_index_style: CandidateIndexStyle::ModernSpace,
            candidate_window_mode: CandidateWindowMode::default(),
            candidate_density: CandidateDensity::default(),
            candidate_theme: CandidateThemePreset::default(),
            candidate_item_spacing: default_candidate_item_spacing(),
            candidate_padding_v: default_candidate_padding_v(),
            candidate_padding_h: default_candidate_padding_h(),
            candidate_corner_radius: default_candidate_corner_radius(),
            candidate_opacity: default_candidate_opacity(),
            show_pinyin_comment: true,
        }
    }
}

impl UiConfig {
    /// 校验并钳制视觉参数边界，防止手工修改出非法数值引发除零或渲染异常
    pub fn sanitize(&mut self) {
        self.page_size = self.page_size.clamp(5, 9);
        if !self.font_size.is_finite() {
            self.font_size = 15.0;
        } else {
            self.font_size = self.font_size.clamp(12.0, 24.0);
        }
        if !self.candidate_item_spacing.is_finite() {
            self.candidate_item_spacing = default_candidate_item_spacing();
        } else {
            self.candidate_item_spacing = self.candidate_item_spacing.clamp(2.0, 24.0);
        }
        if !self.candidate_padding_v.is_finite() {
            self.candidate_padding_v = default_candidate_padding_v();
        } else {
            self.candidate_padding_v = self.candidate_padding_v.clamp(2.0, 16.0);
        }
        if !self.candidate_padding_h.is_finite() {
            self.candidate_padding_h = default_candidate_padding_h();
        } else {
            self.candidate_padding_h = self.candidate_padding_h.clamp(4.0, 24.0);
        }
        if !self.candidate_corner_radius.is_finite() {
            self.candidate_corner_radius = default_candidate_corner_radius();
        } else {
            self.candidate_corner_radius = self.candidate_corner_radius.clamp(0.0, 20.0);
        }
        self.candidate_opacity = self.candidate_opacity.clamp(70, 100);
    }
}

/// 快捷键与按键操作配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ShortcutConfig {
    /// 中英文切换按键
    pub switch_key: SwitchKey,
    /// 翻页快捷键
    pub page_keys: PageKeys,
    /// 简繁体即时切换快捷键（如 "Ctrl + Shift + F"、"已禁用"）
    #[serde(deserialize_with = "deserialize_traditional_shortcut")]
    pub traditional_shortcut: String,
    /// 字符全角/半角切换快捷键（如 "Shift + Space"、"已禁用"）
    #[serde(deserialize_with = "deserialize_full_width_char_shortcut")]
    pub full_width_char_shortcut: String,
    /// 标点全角/半角切换快捷键（如 "Ctrl + ."、"已禁用"）
    #[serde(deserialize_with = "deserialize_full_width_punct_shortcut")]
    pub full_width_punct_shortcut: String,
    /// 是否允许使用 Shift 键选择次选词
    pub shift_select_second: bool,
}

fn deserialize_traditional_shortcut<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let normalized = match s.trim() {
        "CtrlShiftF" => "Ctrl + Shift + F".to_string(),
        "CtrlAltF" => "Ctrl + Alt + F".to_string(),
        "CtrlShiftT" => "Ctrl + Shift + T".to_string(),
        "Disabled" => "已禁用".to_string(),
        other => other.to_string(),
    };
    Ok(normalized)
}

fn deserialize_full_width_char_shortcut<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let normalized = match s.trim() {
        "ShiftSpace" => "Shift + Space".to_string(),
        "CtrlShiftSpace" => "Ctrl + Shift + Space".to_string(),
        "Disabled" => "已禁用".to_string(),
        other => other.to_string(),
    };
    Ok(normalized)
}

fn deserialize_full_width_punct_shortcut<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let normalized = match s.trim() {
        "CtrlPeriod" => "Ctrl + .".to_string(),
        "AltPeriod" => "Alt + .".to_string(),
        "Disabled" => "已禁用".to_string(),
        other => other.to_string(),
    };
    Ok(normalized)
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            switch_key: SwitchKey::Shift,
            page_keys: PageKeys::default(),
            traditional_shortcut: "Ctrl + Shift + F".to_string(),
            full_width_char_shortcut: "Shift + Space".to_string(),
            full_width_punct_shortcut: "Ctrl + .".to_string(),
            shift_select_second: false,
        }
    }
}

/// 拼音算法、词库处理与字符输出行为配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PinyinConfig {
    /// 初始输入语言模式是否为中文（true: 中文，false: 英文）
    pub init_chinese: bool,
    /// 字母/数字全角或半角状态（true: 全角，false: 半角）
    pub full_width_char: bool,
    /// 标点符号全角或半角状态（true: 全角中文标点，false: 半角英文标点）
    pub full_width_punct: bool,
    /// 是否动态调整候选词序（自适应词频学习）
    pub dynamic_adjust_freq: bool,
    /// 是否启用简拼（声母缩写检索）
    pub enable_acronym: bool,
    /// 是否开启斜杠转顿号（将 /? 替换为 、）
    pub slash_to_dunhao: bool,
    /// 是否开启反斜杠转顿号（将 \| 键未按 Shift 时的 \ 替换为 、）
    pub backslash_to_dunhao: bool,
    /// 是否开启数字间英文标点保护（如 12:00、3.14）
    pub protect_digits_punctuation: bool,
    /// 是否开启成对符号自动补全
    pub auto_complete_pairs: bool,
    /// 是否启用 V 模式（计算器与快捷符号）
    pub v_mode: bool,
    /// 是否启用 U 模式（拆字与笔画反查）
    pub enable_u_mode: bool,
    /// 是否启用 I 模式（特殊符号与序号选择）
    pub enable_i_mode: bool,
    /// 是否启用模糊拼音主开关
    pub fuzzy_pinyin: bool,
    /// 模糊拼音 10 项细分规则配置
    pub fuzzy_rules: FuzzyPinyinRules,
    /// 拼音纠错主开关与细分规则配置
    pub typo_rules: TypoCorrectionRules,
    /// 是否默认输出繁体字
    pub traditional_chinese: bool,
    /// 是否开启中西文智能空格（盘古之白）
    pub auto_pangu_spacing: bool,
    /// 是否过滤超出常用规范汉字范围的生僻字符
    pub filter_uncommon_charset: bool,
    /// 是否开启多路候选词自动去重
    pub auto_dedup: bool,
}

impl Default for PinyinConfig {
    fn default() -> Self {
        Self {
            init_chinese: true,
            full_width_char: false,
            full_width_punct: true,
            dynamic_adjust_freq: true,
            enable_acronym: true,
            slash_to_dunhao: true,
            backslash_to_dunhao: true,
            protect_digits_punctuation: true,
            auto_complete_pairs: true,
            v_mode: true,
            enable_u_mode: true,
            enable_i_mode: true,
            fuzzy_pinyin: false,
            fuzzy_rules: FuzzyPinyinRules::default(),
            typo_rules: TypoCorrectionRules::default(),
            traditional_chinese: false,
            auto_pangu_spacing: false,
            filter_uncommon_charset: false,
            auto_dedup: true,
        }
    }
}

/// 独立系统托盘显示模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrayDisplayMode {
    /// 自动检测（默认推荐：Wayland 下自动判定，若具备桌面原生指示器则隐藏独立托盘）
    #[default]
    Auto,
    /// 强制开启专属独立托盘
    Always,
    /// 强制关闭独立托盘
    Never,
}

/// 桌面环境集成与系统托盘配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct DesktopConfig {
    /// 自定义托盘图标主题目录或图标所在目录路径
    pub tray_icon_theme_path: Option<String>,
    /// 自定义中文状态托盘图标名称或路径
    pub tray_icon_zh: Option<String>,
    /// 自定义英文状态托盘图标名称或路径
    pub tray_icon_en: Option<String>,
    /// 独立系统托盘显示模式
    pub enable_tray: TrayDisplayMode,
    /// 是否将微澜设为系统唯一独占输入源
    pub exclusive_input_source: bool,
}
