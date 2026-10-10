use super::*;

use waylan_core::Candidate;

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
    assert!(pixmap.is_some());
    let pixmap = pixmap.expect("渲染位图不可为 None");
    assert!(pixmap.width() > 0);
    assert_eq!(pixmap.height(), 38);
}

#[test]
fn test_render_expanded_popup_card() {
    let renderer = CandidateBoxRenderer::new(UiTheme::light());
    let state = CandidateBoxState {
        visible: true,
        preedit: "ri'qi".to_string(),
        candidates: vec![
            Candidate::new("日期", "ri'qi", 10_000),
            Candidate::new("日期", "ri'qi", 999_999)
                .with_sub_candidates(vec!["2026年10月9日".to_string(), "20261009".to_string()]),
        ],
        selected_index: 1,
        expanded_card: waylan_core::ExpandedCardState::Expanded { selected_index: 0 },
        mode_badge: "中".to_string(),
    };

    let pixmap = renderer.render(&state).expect("应成功渲染展开的二级卡片");
    assert!(pixmap.height() > 50);
}

#[test]
fn test_glyph_cache_hit_and_reuse() {
    let renderer = CandidateBoxRenderer::new(UiTheme::light());
    if !renderer.has_font() {
        return;
    }

    renderer.clear_glyph_cache();
    assert_eq!(renderer.glyph_cache_len(), 0);

    let state = CandidateBoxState {
        visible: true,
        preedit: "ce".to_string(),
        candidates: vec![
            Candidate::new("测试", "ce'shi", 100),
            Candidate::new("侧视", "ce'shi", 90),
        ],
        selected_index: 0,
        expanded_card: waylan_core::ExpandedCardState::Collapsed,
        mode_badge: "中".to_string(),
    };

    let _ = renderer.render(&state);
    let cache_len_1 = renderer.glyph_cache_len();
    assert!(cache_len_1 > 0);

    let _ = renderer.render(&state);
    let cache_len_2 = renderer.glyph_cache_len();
    assert_eq!(cache_len_1, cache_len_2, "再次渲染相同候选词应100%命中缓存");
}

#[test]
fn test_hidpi_scale_rendering() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    let state = CandidateBoxState {
        visible: true,
        preedit: "ce".to_string(),
        candidates: vec![Candidate::new("测试", "ce'shi", 100)],
        selected_index: 0,
        expanded_card: waylan_core::ExpandedCardState::Collapsed,
        mode_badge: "中".to_string(),
    };

    let pixmap_1x = renderer.render(&state).expect("1x 渲染失败");
    assert_eq!(pixmap_1x.height(), 38);

    renderer.set_scale(2.0);
    let pixmap_2x = renderer.render(&state).expect("2x 渲染失败");
    assert_eq!(pixmap_2x.height(), 76);
}
