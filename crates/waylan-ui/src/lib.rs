pub mod renderer;
pub mod theme;

pub use renderer::CandidateBoxRenderer;
pub use theme::{Color, UiTheme};

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_core::{Candidate, CandidateBoxState};

    #[test]
    fn test_renderer_basic() {
        let renderer = CandidateBoxRenderer::new(UiTheme::light());
        let state = CandidateBoxState {
            visible: true,
            preedit: "ni'hao".to_string(),
            candidates: vec![
                Candidate::new("你好", "ni'hao", 10000),
                Candidate::new("拟好", "ni'hao", 8000),
                Candidate::new("泥豪", "ni'hao", 6000),
            ],
            selected_index: 0,
            expanded_card: waylan_core::ExpandedCardState::Collapsed,
            mode_badge: "中".to_string(),
        };

        let pixmap = renderer.render(&state);
        assert!(pixmap.is_some(), "应该成功渲染出像素位图");
        let pixmap = pixmap.unwrap();
        assert!(pixmap.width() > 0);
        assert!(pixmap.height() > 0);
    }
}
