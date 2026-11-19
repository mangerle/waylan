mod general;
mod groups;
mod shortcuts;
mod voice;

pub use general::*;
pub use groups::*;
pub use shortcuts::*;
pub use voice::*;

/// 绑定 GUI 下拉框索引的配置枚举双向转换契约
///
/// # 设计原理
/// - **实现初衷**：将配置枚举与 Slint 界面下拉索引（`i32`）的双向映射规则收口到领域模型旁，
///   消除上层设置界面同步模块中大量的手工 `match idx` 样板分支与映射漂移风险。
/// - **核心优势**：单一真实源，编译期覆盖全部分支，越界索引自动安全回退默认值。
pub trait UiIndexEnum: Sized {
    /// 转换为界面下拉框索引（0-indexed）
    fn as_ui_index(&self) -> i32;
    /// 从界面下拉框索引解析枚举值，越界时安全回退为默认值
    fn from_ui_index(index: i32) -> Self;
}

/// 为配置枚举生成 `UiIndexEnum` 双向索引映射实现
macro_rules! impl_ui_index_enum {
    ($ty:ty, default: $default_variant:expr, { $($variant:path => $idx:literal),+ $(,)? }) => {
        impl UiIndexEnum for $ty {
            #[inline]
            fn as_ui_index(&self) -> i32 {
                match self {
                    $( $variant => $idx, )+
                }
            }

            #[inline]
            fn from_ui_index(index: i32) -> Self {
                match index {
                    $( $idx => $variant, )+
                    _ => $default_variant,
                }
            }
        }
    };
}

impl_ui_index_enum!(SwitchKey, default: SwitchKey::Shift, {
    SwitchKey::Shift => 0,
    SwitchKey::Ctrl => 1,
});

impl_ui_index_enum!(CandidateWindowMode, default: CandidateWindowMode::WaylanCustom, {
    CandidateWindowMode::WaylanCustom => 0,
    CandidateWindowMode::Native => 1,
});

impl_ui_index_enum!(CandidateDensity, default: CandidateDensity::Normal, {
    CandidateDensity::Compact => 0,
    CandidateDensity::Normal => 1,
    CandidateDensity::Spacious => 2,
});

impl_ui_index_enum!(CandidateThemePreset, default: CandidateThemePreset::Emerald, {
    CandidateThemePreset::Emerald => 0,
    CandidateThemePreset::Monochrome => 1,
    CandidateThemePreset::IceBlue => 2,
    CandidateThemePreset::WarmLatte => 3,
});

impl_ui_index_enum!(CandidateOrientation, default: CandidateOrientation::Horizontal, {
    CandidateOrientation::Horizontal => 0,
    CandidateOrientation::Vertical => 1,
});

impl_ui_index_enum!(CandidateIndexStyle, default: CandidateIndexStyle::ModernSpace, {
    CandidateIndexStyle::ModernSpace => 0,
    CandidateIndexStyle::ClassicDot => 1,
    CandidateIndexStyle::Circled => 2,
    CandidateIndexStyle::SquareBracket => 3,
    CandidateIndexStyle::Parenthesis => 4,
    CandidateIndexStyle::MinimalFirstDot => 5,
    CandidateIndexStyle::None => 6,
});

impl_ui_index_enum!(VoiceVisualEffect, default: VoiceVisualEffect::DynamicWaveform, {
    VoiceVisualEffect::DynamicWaveform => 0,
    VoiceVisualEffect::BreathingPulse => 1,
    VoiceVisualEffect::ClassicText => 2,
});

impl UiIndexEnum for NsEngineType {
    #[inline]
    fn as_ui_index(&self) -> i32 {
        match self {
            Self::Rnnoise | Self::Bypass => 0,
            Self::DeepFilterNet => 1,
            Self::Gtcrn => 2,
        }
    }

    #[inline]
    fn from_ui_index(index: i32) -> Self {
        match index {
            1 => Self::DeepFilterNet,
            2 => Self::Gtcrn,
            _ => Self::Rnnoise,
        }
    }
}

#[cfg(test)]
mod ui_index_tests {
    use super::*;

    #[test]
    fn test_ui_index_enum_roundtrips() {
        for (i, expected) in [(0, SwitchKey::Shift), (1, SwitchKey::Ctrl)] {
            assert_eq!(expected.as_ui_index(), i);
            assert_eq!(SwitchKey::from_ui_index(i), expected);
        }
        assert_eq!(SwitchKey::from_ui_index(99), SwitchKey::Shift);

        for (i, expected) in [
            (0, CandidateDensity::Compact),
            (1, CandidateDensity::Normal),
            (2, CandidateDensity::Spacious),
        ] {
            assert_eq!(expected.as_ui_index(), i);
            assert_eq!(CandidateDensity::from_ui_index(i), expected);
        }
        assert_eq!(
            CandidateDensity::from_ui_index(-1),
            CandidateDensity::Normal
        );

        for (i, expected) in [
            (0, CandidateIndexStyle::ModernSpace),
            (1, CandidateIndexStyle::ClassicDot),
            (2, CandidateIndexStyle::Circled),
            (3, CandidateIndexStyle::SquareBracket),
            (4, CandidateIndexStyle::Parenthesis),
            (5, CandidateIndexStyle::MinimalFirstDot),
            (6, CandidateIndexStyle::None),
        ] {
            assert_eq!(expected.as_ui_index(), i);
            assert_eq!(CandidateIndexStyle::from_ui_index(i), expected);
        }

        for (i, expected) in [
            (0, VoiceVisualEffect::DynamicWaveform),
            (1, VoiceVisualEffect::BreathingPulse),
            (2, VoiceVisualEffect::ClassicText),
        ] {
            assert_eq!(expected.as_ui_index(), i);
            assert_eq!(VoiceVisualEffect::from_ui_index(i), expected);
        }
    }
}
