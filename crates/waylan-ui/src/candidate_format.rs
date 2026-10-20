//! 候选词展示文本与序号一体化排版格式化模块（重导出权威核心实现）
//!
//! # 设计原理
//! - **实现初衷**：候选词序号排版与格式化属于领域纯文本排版逻辑，核心权威实现已下沉至 `waylan-core`。
//! - **核心优势**：本模块作为外观表现层保持兼容性重导出，切断非 GUI 协议端对 UI 库的循环与污染依赖。

pub use waylan_core::candidate::{
    format_candidate_display, format_candidate_index, CIRCLED_NUMBERS,
};

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::CandidateIndexStyle;

    #[test]
    fn test_ui_candidate_format_reexport() {
        assert_eq!(
            format_candidate_display(CandidateIndexStyle::ModernSpace, 0, "微澜", None),
            "1 微澜"
        );
    }
}
