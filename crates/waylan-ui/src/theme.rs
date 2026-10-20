//! 候选框主题颜色与圆角内边距定义

use waylan_core::config::CandidateThemePreset;
use waylan_core::ThemeMode;

/// 颜色定义 (RGBA，每个分量为 0.0 ~ 1.0)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// 由 0.0~1.0 浮点四通道值构造颜色
    pub const fn from_rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// 由 24 位十六进制颜色与独立透明度通道构造颜色
    pub const fn from_hex(hex: u32, alpha: f32) -> Self {
        let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
        let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
        let b = (hex & 0xFF) as f32 / 255.0;
        Self { r, g, b, a: alpha }
    }

    /// 转换为 tiny-skia 的 Color 结构体
    pub fn to_tiny_skia(&self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba(self.r, self.g, self.b, self.a)
            .unwrap_or(tiny_skia::Color::BLACK)
    }
}

/// 候选框界面主题配置
#[derive(Debug, Clone)]
pub struct UiTheme {
    /// 候选框背景色
    pub background: Color,
    /// 边框颜色
    pub border: Color,
    /// 预编辑拼音文字颜色
    pub preedit_text: Color,
    /// 首选词高亮胶囊背景色
    pub highlight_badge: Color,
    /// 首选词文字颜色
    pub highlight_text: Color,
    /// 普通候选词文字颜色
    pub candidate_text: Color,
    /// 序号文字颜色 (如 "2.", "3.")
    pub index_text: Color,
    /// 边框圆角半径 (像素)
    pub corner_radius: f32,
    /// 内边距 (像素)
    pub padding: f32,
}

impl UiTheme {
    /// 浅色优雅主题（对标现代高品质输入法：白底青翠首选）
    pub fn light() -> Self {
        Self {
            background: Color::from_hex(0xFFFFFF, 0.98),
            border: Color::from_hex(0xE2E8F0, 1.0),
            preedit_text: Color::from_hex(0x0F172A, 1.0),
            highlight_badge: Color::from_hex(0x07C160, 1.0), // 翡翠绿
            highlight_text: Color::from_hex(0xFFFFFF, 1.0),
            candidate_text: Color::from_hex(0x1E293B, 1.0),
            index_text: Color::from_hex(0x94A3B8, 1.0),
            corner_radius: 10.0,
            padding: 10.0,
        }
    }

    /// 深色夜间主题
    pub fn dark() -> Self {
        Self {
            background: Color::from_hex(0x18181B, 0.96),
            border: Color::from_hex(0x27272A, 1.0),
            preedit_text: Color::from_hex(0xF4F4F5, 1.0),
            highlight_badge: Color::from_hex(0x059669, 1.0),
            highlight_text: Color::from_hex(0xFFFFFF, 1.0),
            candidate_text: Color::from_hex(0xE4E4E7, 1.0),
            index_text: Color::from_hex(0x71717A, 1.0),
            corner_radius: 10.0,
            padding: 10.0,
        }
    }

    /// 根据主题模式与色彩预设构造主题
    pub fn from_config(mode: ThemeMode, preset: CandidateThemePreset) -> Self {
        let mut theme = match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
        };
        let dark = matches!(mode, ThemeMode::Dark);
        match preset {
            CandidateThemePreset::Emerald => {}
            CandidateThemePreset::Monochrome => {
                theme.highlight_badge = if dark {
                    Color::from_hex(0xF4F4F5, 1.0)
                } else {
                    Color::from_hex(0x18181B, 1.0)
                };
                theme.highlight_text = if dark {
                    Color::from_hex(0x18181B, 1.0)
                } else {
                    Color::from_hex(0xFFFFFF, 1.0)
                };
            }
            CandidateThemePreset::IceBlue => {
                theme.highlight_badge = if dark {
                    Color::from_hex(0x0369A1, 1.0)
                } else {
                    Color::from_hex(0x0284C7, 1.0)
                };
            }
            CandidateThemePreset::WarmLatte => {
                theme.highlight_badge = if dark {
                    Color::from_hex(0xB45309, 1.0)
                } else {
                    Color::from_hex(0xD97706, 1.0)
                };
            }
        }
        theme
    }
}
