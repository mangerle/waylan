//! 文本规整与语气词过滤流水线 (Inverse Text Normalization - ITN)
//!
//! # 设计原理
//! - **实现初衷**：在 ASR 语音识别与标点预测生成文本后，提供口语停顿词消除、数字转阿拉伯数字与中英文盘古空格规整。
//! - **核心优势**：职责清晰的三阶段流水线设计（语气词消除 -> 数字标准化 -> 智能盘古空格），零外部模型依赖。
//! - **代价与局限**：阶段执行顺序固定，依赖前序清洗结果。

pub mod filler;
pub mod numbers;
pub mod pangu;

pub use filler::filter_filler_words;
pub use numbers::convert_chinese_numbers;
pub use pangu::apply_pangu_spacing;

use waylan_core::VoiceItnConfig;

/// 综合执行文本规整流水线（语气词消除 -> 数字转阿拉伯 -> 盘古空格）
pub fn normalize_voice_text(raw: &str, config: &VoiceItnConfig) -> String {
    let mut text = raw.trim().to_string();
    if text.is_empty() {
        return text;
    }

    // 1. 口语停顿语气词与结巴消除
    if config.filter_filler_words {
        text = filter_filler_words(&text);
    }

    // 2. 逆文本标准化：大写数字转阿拉伯数字
    if config.convert_numbers {
        text = convert_chinese_numbers(&text);
    }

    // 3. 中英文自动盘古空格 (风格 B 优雅紧凑)
    if config.pangu_spacing {
        text = apply_pangu_spacing(&text);
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_pipeline_normalization() {
        let cfg = VoiceItnConfig {
            convert_numbers: true,
            filter_filler_words: true,
            pangu_spacing: true,
        };

        let raw = "呃，我们在二零二六年用Rust完成了百分之五十的目标。";
        let norm = normalize_voice_text(raw, &cfg);
        assert_eq!(norm, "我们在 2026年用 Rust 完成了 50%的目标。");
    }
}
