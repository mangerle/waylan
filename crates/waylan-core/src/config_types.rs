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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

/// 辅助反序列化枚举，同时支持新版结构体与旧版字符串枚举两种形态，保障版本平滑过渡
#[derive(Deserialize)]
#[serde(untagged)]
enum PageKeysHelper {
    Struct {
        #[serde(default)]
        minus_equal: bool,
        #[serde(default)]
        bracket: bool,
        #[serde(default)]
        comma_period: bool,
    },
    LegacyStr(String),
}

impl<'de> Deserialize<'de> for PageKeys {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match PageKeysHelper::deserialize(deserializer)? {
            PageKeysHelper::Struct {
                minus_equal,
                bracket,
                comma_period,
            } => Ok(Self {
                minus_equal,
                bracket,
                comma_period,
            }),
            PageKeysHelper::LegacyStr(s) => match s.as_str() {
                "MinusEqual" => Ok(Self {
                    minus_equal: true,
                    bracket: false,
                    comma_period: false,
                }),
                "Bracket" => Ok(Self {
                    minus_equal: false,
                    bracket: true,
                    comma_period: false,
                }),
                "CommaPeriod" => Ok(Self {
                    minus_equal: false,
                    bracket: false,
                    comma_period: true,
                }),
                "PageUpPageDown" => Ok(Self {
                    minus_equal: false,
                    bracket: false,
                    comma_period: false,
                }),
                _ => Ok(Self::default()),
            },
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

/// 模糊音细分规则配置（对应模糊音设置页面 10 项开关）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// en <=> eng
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

impl Default for FuzzyPinyinRules {
    fn default() -> Self {
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
}

/// 拼音纠错细分规则配置（对应拼音模式页面拼音纠错开关）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
