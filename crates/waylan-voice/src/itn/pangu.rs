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
    fn test_pangu_spacing_style_b() {
        // 1. 中英文相连加空格
        assert_eq!(apply_pangu_spacing("用Rust编写微澜"), "用 Rust 编写微澜");
        assert_eq!(
            apply_pangu_spacing("微澜WayLan输入法"),
            "微澜 WayLan 输入法"
        );

        // 2. 中文全角标点不加多余空格
        assert_eq!(
            apply_pangu_spacing("使用Rust，编写WayLan。"),
            "使用 Rust，编写 WayLan。"
        );

        // 3. 数字与量词/时间词保持紧凑
        assert_eq!(
            apply_pangu_spacing("我在2026年买了1台电脑"),
            "我在 2026年买了 1台电脑"
        );
        assert_eq!(apply_pangu_spacing("降价了50%"), "降价了 50%");

        // 4. 汉字与独立数字加空格
        assert_eq!(apply_pangu_spacing("版本2发布"), "版本 2 发布");

        // 5. 汉字零“〇”应正确识别为 CJK 汉字
        assert_eq!(apply_pangu_spacing("二〇二六年Rust"), "二〇二六年 Rust");
    }
}
