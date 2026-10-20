use serde::{Deserialize, Serialize};

/// 中英切换键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SwitchKey {
    #[default]
    Shift,
    Ctrl,
}

/// 主题模式选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

/// 候选翻页快捷键配置（支持多选同时生效）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageKeys {
    /// 减号与等号 (- / =)
    pub minus_equal: bool,
    /// 左右中括号 ([ / ])
    pub bracket: bool,
    /// 逗号与句号 (, / .)
    pub comma_period: bool,
}

impl Default for PageKeys {
    fn default() -> Self {
        Self {
            minus_equal: true,
            bracket: false,
            comma_period: false,
        }
    }
}

/// 候选框排列方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CandidateOrientation {
    /// 水平排列（默认）
    #[default]
    Horizontal,
    /// 垂直排列
    Vertical,
}

/// 候选词序号展示风格
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CandidateIndexStyle {
    /// 现代空白风：1 候选词（纯数字 + 半角空格，最干净利落，消除点号噪点）
    #[default]
    ModernSpace,
    /// 经典点号风：1. 候选词（传统输入法习惯）
    ClassicDot,
    /// 圆圈序号风：① 候选词（优雅紧凑，非常适合中文字形）
    Circled,
    /// 括号提示风：[1] 候选词
    SquareBracket,
    /// 圆括号提示风：(1) 候选词
    Parenthesis,
    /// 全极简无序号：仅在首选词加小圆点标记，后续词纯文本
    MinimalFirstDot,
    /// 纯文本完全无序号（适合纯空格选词的盲打用户）
    None,
}

/// 候选框渲染引擎模式（适用于 GNOME / IBus 桌面环境）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CandidateWindowMode {
    /// 微澜现代专属候选框（默认推荐，支持精致胶囊、色彩主题与排版密度）
    #[default]
    WaylanCustom,
    /// 桌面系统原生候选框（完全交由桌面环境与系统主题原生渲染，保留同基线与隐藏多余序号修复）
    Native,
}

/// 候选框排版空间密度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDensity {
    /// 紧凑极简模式：极小留白，专为编码与小屏幕设计
    Compact,
    /// 标准平衡模式：舒适呼吸感（默认）
    #[default]
    Normal,
    /// 宽松舒缓模式：较大留白与圆角，适合高分大屏
    Spacious,
}

/// 候选框色彩主题预设
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CandidateThemePreset {
    /// 微澜翡翠绿（经典官方主题，浅色/深色自适应，默认）
    #[default]
    Emerald,
    /// 极简黑白墨韵（纯高对比黑白灰）
    Monochrome,
    /// 极客暗夜冰蓝（霓虹冰蓝高亮）
    IceBlue,
    /// 暖阳奶茶（温润暖色米杏与焦糖橙）
    WarmLatte,
}

/// 模糊音细分规则配置（对应模糊音设置页面 10 项开关）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FuzzyPinyinRules {
    /// c <=> ch
    pub c_ch: bool,
    /// z <=> zh
    pub z_zh: bool,
    /// s <=> sh
    pub s_sh: bool,
    /// an <=> ang
    pub an_ang: bool,
    /// 前后鼻音: en <=> eng
    pub en_eng: bool,
    /// in <=> ing
    pub in_ing: bool,
    /// l <=> n
    pub l_n: bool,
    /// f <=> h
    pub f_h: bool,
    /// l <=> r
    pub r_l: bool,
    /// g <=> k
    pub g_k: bool,
}

impl FuzzyPinyinRules {
    /// 推荐常用模糊音预设组合（常用平翘舌、前后鼻音、边鼻音）
    pub fn recommended() -> Self {
        Self {
            c_ch: true,
            z_zh: true,
            s_sh: true,
            an_ang: true,
            en_eng: true,
            in_ing: true,
            l_n: true,
            f_h: true,
            r_l: false,
            g_k: false,
        }
    }
    /// 创建开启全部 10 类模糊音规则的配置
    pub fn all_enabled() -> Self {
        Self {
            c_ch: true,
            z_zh: true,
            s_sh: true,
            an_ang: true,
            en_eng: true,
            in_ing: true,
            l_n: true,
            f_h: true,
            r_l: true,
            g_k: true,
        }
    }

    /// 检查是否存在任一激活的模糊音规则
    pub fn has_active_rules(&self) -> bool {
        self.c_ch
            || self.z_zh
            || self.s_sh
            || self.an_ang
            || self.en_eng
            || self.in_ing
            || self.l_n
            || self.f_h
            || self.r_l
            || self.g_k
    }
}

/// 拼音纠错细分规则配置（对应拼音模式页面拼音纠错开关）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TypoCorrectionRules {
    /// 是否启用拼音纠错主开关
    pub enable: bool,
    /// gn => ng
    pub gn_ng: bool,
    /// mg => ng
    pub mg_ng: bool,
    /// iou => iu
    pub iou_iu: bool,
    /// uei => ui
    pub uei_ui: bool,
    /// uen => un
    pub uen_un: bool,
    /// ue => ve
    pub ue_ve: bool,
    /// v => u, ve => ue, van => uan 等
    pub v_u: bool,
    /// on => ong
    pub on_ong: bool,
}

impl Default for TypoCorrectionRules {
    fn default() -> Self {
        Self {
            enable: true,
            gn_ng: true,
            mg_ng: true,
            iou_iu: true,
            uei_ui: true,
            uen_un: true,
            ue_ve: true,
            v_u: true,
            on_ong: true,
        }
    }
}

/// 简繁体即时切换快捷键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TraditionalShortcut {
    #[default]
    CtrlShiftF,
    CtrlAltF,
    CtrlShiftT,
    Disabled,
}

/// 字符全角/半角切换快捷键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FullWidthCharShortcut {
    #[default]
    ShiftSpace,
    CtrlShiftSpace,
    Disabled,
}

/// 标点全角/半角切换快捷键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FullWidthPunctShortcut {
    #[default]
    CtrlPeriod,
    AltPeriod,
    Disabled,
}
