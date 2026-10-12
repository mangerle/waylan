use super::*;

use waylan_core::{Candidate, ExpandedCardState};

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
        expanded_card: ExpandedCardState::Collapsed,
        mode_badge: "中".to_string(),
        is_grid_expanded: false,
        grid_candidates: Vec::new(),
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
        expanded_card: ExpandedCardState::Expanded { selected_index: 0 },
        mode_badge: "中".to_string(),
        is_grid_expanded: false,
        grid_candidates: Vec::new(),
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
        expanded_card: ExpandedCardState::Collapsed,
        mode_badge: "中".to_string(),
        is_grid_expanded: false,
        grid_candidates: Vec::new(),
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
        expanded_card: ExpandedCardState::Collapsed,
        mode_badge: "中".to_string(),
        is_grid_expanded: false,
        grid_candidates: Vec::new(),
    };

    let pixmap_1x = renderer.render(&state).expect("1x 渲染失败");
    assert_eq!(pixmap_1x.height(), 38);

    renderer.set_scale(2.0);
    let pixmap_2x = renderer.render(&state).expect("2x 渲染失败");
    assert_eq!(pixmap_2x.height(), 76);
}

#[test]
fn test_fallback_font_chain() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    assert!(renderer.has_font(), "默认应至少加载系统可用字体");

    // 验证 get_font_for_char 能够为常规中文字符与英文字符定位有效字体
    let font_cn = renderer.get_font_for_char('中');
    assert!(font_cn.is_some(), "应能为主中文字符返回可用字体");

    let font_en = renderer.get_font_for_char('A');
    assert!(font_en.is_some(), "应能为英文 ASCII 字符返回可用字体");

    // 若系统已存在回退字体（如 Noto Color Emoji 或 DejaVuSans），验证回退字体链已纳管
    let fb_fonts = FontManager::load_fallback_fonts();
    let fb_fonts_2 = FontManager::load_fallback_fonts();
    assert!(
        Arc::ptr_eq(&fb_fonts, &fb_fonts_2),
        "FontManager::load_fallback_fonts 应返回同一 Arc 共享句柄"
    );
    for fb in fb_fonts.iter() {
        renderer.add_fallback_font(fb.clone());
    }
    assert!(renderer.has_font());
}

#[test]
fn test_glyph_cache_lru_eviction() {
    let renderer = CandidateBoxRenderer::new(UiTheme::light());
    if !renderer.has_font() {
        return;
    }
    renderer.clear_glyph_cache();

    // 预热一个高频热点字符 '中'
    renderer.measure_text("中", 15.0);
    assert_eq!(renderer.glyph_cache_len(), 1);

    // 持续测量大量不同字符（触发缓存超过 2048 上限与 LRU 淘汰）
    for i in 0..2100 {
        if let Some(ch) = char::from_u32(0x4E00 + i) {
            renderer.measure_text(&ch.to_string(), 15.0);
            // 周期性重新访问高频热点字符 '中'，刷新其 LRU 时钟
            if i % 50 == 0 {
                renderer.measure_text("中", 15.0);
            }
        }
    }

    // 验证总容量被严格限制在 MAX_GLYPH_CACHE_SIZE 内
    let len = renderer.glyph_cache_len();
    assert!(len <= 2048, "字形缓存容量应被严格限制在上限内");
    assert!(len > 1500, "淘汰后应保留大部分活跃字形");

    // 验证频繁访问的热点字形在淘汰后依然被保留在缓存中（命中缓存，len 不变）
    let before_len = renderer.glyph_cache_len();
    renderer.measure_text("中", 15.0);
    assert_eq!(
        renderer.glyph_cache_len(),
        before_len,
        "高频字符应因 LRU 保护常驻缓存"
    );
}

#[test]
fn test_render_grid_expanded_and_hit_test() {
    let renderer = CandidateBoxRenderer::new(UiTheme::light());
    let mut grid_cands = Vec::with_capacity(30);
    for i in 0..30 {
        grid_cands.push(Candidate::new(format!("候选{}", i + 1), "hou'xuan", 100));
    }

    let state = CandidateBoxState {
        visible: true,
        preedit: "hou'xuan".to_string(),
        candidates: grid_cands.iter().take(6).cloned().collect(),
        selected_index: 0,
        expanded_card: ExpandedCardState::Collapsed,
        mode_badge: String::new(),
        is_grid_expanded: true,
        grid_candidates: grid_cands,
    };

    let pixmap = renderer.render(&state).expect("5行网格面板应渲染成功");
    assert!(pixmap.height() >= 140, "5行网格面板高度应至少140像素");
    assert!(pixmap.width() >= 300, "5行网格面板宽度应充足以容纳6列");

    // 测试右上角折叠按钮命中测试
    let collapse_target = renderer.hit_test(pixmap.width() as f32 - 14.0, 20.0);
    assert_eq!(collapse_target, Some(CandidateHitTarget::ToggleExpand));

    // 测试首个候选词命中测试
    let first_cand_target = renderer.hit_test(20.0, 20.0);
    assert_eq!(first_cand_target, Some(CandidateHitTarget::Select(0)));
}
