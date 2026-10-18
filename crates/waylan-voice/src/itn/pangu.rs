//! 语音排版盘古空格算法（风格 B：优雅紧凑）
//!
//! # 设计原理
//! - **实现初衷**：在语音识别结果中自动协调中文汉字与西文字母、数字的排版间隙，提供更符合现代中文排版美学的文本呈现。
//! - **核心优势**：直接复用 `waylan_core::apply_pangu_spacing` 核心权威实现，消除双重维护。

pub use waylan_core::apply_pangu_spacing;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voice_pangu_export() {
        assert_eq!(apply_pangu_spacing("微澜WayLan"), "微澜 WayLan");
    }
}
