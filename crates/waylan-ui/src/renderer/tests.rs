use super::*;

use waylan_core::{Candidate, CandidateExpansion};

#[test]
fn test_renderer_basic() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    let state = CandidateBoxState {
        visible: true,
        preedit: "ni'hao".to_string(),
        candidates: vec![
            Candidate::new("你好", "ni'hao", 10000),
            Candidate::new("拟好", "ni'hao", 8000),
            Candidate::new("泥豪", "ni'hao", 6000),
        ],
        selected_index: 0,
        expansion: CandidateExpansion::None,
        mode_badge: "中".to_string(),
        grid_candidates: Vec::new(),
        voice: None,
    };

    let pixmap = renderer.render(&state);
    assert!(pixmap.is_some());
    let pixmap = pixmap.expect("渲染位图不可为 None");
    assert!(pixmap.width() > 0);
    assert_eq!(pixmap.height(), 38);
}

#[test]
fn test_render_expanded_popup_card() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    let state = CandidateBoxState {
        visible: true,
        preedit: "ri'qi".to_string(),
        candidates: vec![
            Candidate::new("日期", "ri'qi", 10_000),
            Candidate::new("日期", "ri'qi", 999_999)
                .with_sub_candidates(vec!["2026年10月9日".to_string(), "20261009".to_string()]),
        ],
        selected_index: 1,
        expansion: CandidateExpansion::Card { selected_index: 0 },
        mode_badge: "中".to_string(),
        grid_candidates: Vec::new(),
        voice: None,
    };

    let pixmap = renderer.render(&state).expect("应成功渲染展开的二级卡片");
    assert!(pixmap.height() > 50);
}

#[test]
fn test_glyph_cache_hit_and_reuse() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    if !renderer.has_font() {
        return;
    }

    let state = CandidateBoxState {
        visible: true,
        preedit: "ce".to_string(),
        candidates: vec![
            Candidate::new("测试", "ce'shi", 100),
            Candidate::new("侧视", "ce'shi", 90),
        ],
        selected_index: 0,
        expansion: CandidateExpansion::None,
        mode_badge: "中".to_string(),
        grid_candidates: Vec::new(),
        voice: None,
    };

    let pixmap_1 = renderer.render(&state);
    assert!(pixmap_1.is_some());

    let pixmap_2 = renderer.render(&state);
    assert!(pixmap_2.is_some());

    assert_eq!(
        pixmap_1.unwrap().width(),
        pixmap_2.unwrap().width(),
        "重复渲染同一候选词状态尺寸应完全一致"
    );
}

#[test]
fn test_hidpi_scale_rendering() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    let state = CandidateBoxState {
        visible: true,
        preedit: "ce".to_string(),
        candidates: vec![Candidate::new("测试", "ce'shi", 100)],
        selected_index: 0,
        expansion: CandidateExpansion::None,
        mode_badge: "中".to_string(),
        grid_candidates: Vec::new(),
        voice: None,
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

    // 验证 cosmic-text 能对常规中文字符、英文字符与特殊符号正常测量
    let w_cn = renderer.measure_text("中文测试", 15.0);
    assert!(w_cn > 0.0, "中文测量宽度应大于0");

    let w_en = renderer.measure_text("Hello World", 15.0);
    assert!(w_en > 0.0, "英文测量宽度应大于0");

    let w_sym = renderer.measure_text("★※→", 15.0);
    assert!(w_sym > 0.0, "特殊符号测量宽度应大于0");
}

#[test]
fn test_glyph_cache_lru_eviction() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    if !renderer.has_font() {
        return;
    }
    renderer.clear_glyph_cache();

    // 预热一个高频热点字符 '中'
    let w_init = renderer.measure_text("中", 15.0);
    assert!(w_init > 0.0);

    // 持续测量大量不同字符（验证大规模文本测量稳定不 panic）
    for i in 0..500 {
        if let Some(ch) = char::from_u32(0x4E00 + i) {
            let _ = renderer.measure_text(&ch.to_string(), 15.0);
        }
    }

    let w_after = renderer.measure_text("中", 15.0);
    assert_eq!(w_init, w_after, "高频字符在多轮测量后宽度应严格一致");
}

#[test]
fn test_render_grid_expanded_and_hit_test() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    let mut grid_cands = Vec::with_capacity(30);
    for i in 0..30 {
        grid_cands.push(Candidate::new(format!("候选{}", i + 1), "hou'xuan", 100));
    }

    let state = CandidateBoxState {
        visible: true,
        preedit: "hou'xuan".to_string(),
        candidates: grid_cands.iter().take(6).cloned().collect(),
        selected_index: 0,
        expansion: CandidateExpansion::Grid { selected_index: 0 },
        mode_badge: String::new(),
        grid_candidates: grid_cands,
        voice: None,
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

#[test]
fn test_cosmic_text_smoke() {
    use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache};

    let mut font_system = FontSystem::new();
    let mut swash_cache = SwashCache::new();
    let font_size = 15.0;
    let metrics = Metrics::new(font_size, font_size * 1.2);
    let mut buffer = Buffer::new(&mut font_system, metrics);
    buffer.set_text(
        "你好世界 Hello",
        &Attrs::new().family(Family::SansSerif),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);
    let mut line_w: f32 = 0.0;
    let baseline_y = 25.0f32;
    let start_x = 10.0f32;

    for run in buffer.layout_runs() {
        line_w = line_w.max(run.line_w);
        for glyph in run.glyphs {
            let physical = glyph.physical((start_x, baseline_y - run.line_y), 1.0);
            if let Some(img) = swash_cache.get_image(&mut font_system, physical.cache_key)
                && img.placement.width > 0
                && img.placement.height > 0
            {
                let _gx = physical.x + img.placement.left;
                let _gy = physical.y - img.placement.top;
                assert!(!img.data.is_empty());
            }
        }
    }
    assert!(line_w > 0.0);
}

#[test]
fn test_measure_text_cache_hit_and_clear() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    let w1 = renderer.measure_text("测试", 15.0);
    let w2 = renderer.measure_text("测试", 15.0);
    assert_eq!(w1, w2, "缓存命中的测量宽度必须完全一致");

    renderer.clear_glyph_cache();
    let w3 = renderer.measure_text("测试", 15.0);
    assert_eq!(w1, w3, "清空缓存后重新计算的测量宽度必须保持完全一致");
}

#[test]
fn test_render_voice_capsule_bar() {
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    let state = CandidateBoxState {
        visible: true,
        preedit: "你好".to_string(),
        candidates: Vec::new(),
        selected_index: 0,
        expansion: CandidateExpansion::None,
        mode_badge: String::new(),
        grid_candidates: Vec::new(),
        voice: Some(waylan_core::VoiceBoxState {
            partial_text: "正在识别中...".to_string(),
            waveform: " ▃▅▇█▇▅▃ ".to_string(),
            display_text: None,
            is_listening: true,
            ..Default::default()
        }),
    };

    assert!(state.is_voice_active(), "语音状态必须处于激活");
    let pixmap = renderer.render(&state);
    assert!(
        pixmap.is_some(),
        "即使 candidates 为空也必须成功渲染语音胶囊"
    );
    let pixmap = pixmap.expect("语音胶囊位图不可为空");
    assert!(pixmap.width() >= 140, "语音胶囊卡片宽度应满足最小阈值");
    assert_eq!(pixmap.height(), 38, "默认 1x 下语音胶囊高度为 38 像素");
}

#[test]
fn test_renderer_applies_ui_config_and_index_style() {
    use waylan_core::ThemeMode;
    use waylan_core::config::{
        CandidateDensity, CandidateIndexStyle, CandidateThemePreset, UiConfig,
    };

    let ui = UiConfig {
        candidate_density: CandidateDensity::Compact,
        candidate_theme: CandidateThemePreset::IceBlue,
        candidate_index_style: CandidateIndexStyle::Circled,
        candidate_item_spacing: 12.0,
        candidate_padding_h: 14.0,
        candidate_corner_radius: 10.0,
        candidate_opacity: 90,
        font_size: 18.0,
        ..Default::default()
    };
    let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
    renderer.apply_ui_config(&ui, ThemeMode::Dark);

    assert_eq!(renderer.font_size, 18.0);
    assert_eq!(renderer.style.item_gap, 12.0);
    assert_eq!(renderer.style.padding_h, 14.0);
    assert_eq!(renderer.index_label(0), "①\u{2009}");
    assert!(renderer.theme.background.a <= 0.96 * 0.9 + f32::EPSILON);

    renderer.style.show_pinyin_comment = true;
    let candidate = Candidate::new("你好", "ni'hao", 100).with_comment("[ni'hao]");
    assert_eq!(renderer.display_text(&candidate), "你好 [ni'hao]");
}

#[test]
fn test_voice_visual_effect_variants_render() {
    use waylan_core::VoiceVisualEffect;

    for effect in [
        VoiceVisualEffect::DynamicWaveform,
        VoiceVisualEffect::BreathingPulse,
        VoiceVisualEffect::ClassicText,
    ] {
        let mut renderer = CandidateBoxRenderer::new(UiTheme::light());
        let state = CandidateBoxState {
            visible: true,
            preedit: String::new(),
            candidates: Vec::new(),
            selected_index: 0,
            expansion: CandidateExpansion::None,
            mode_badge: String::new(),
            grid_candidates: Vec::new(),
            voice: Some(waylan_core::VoiceBoxState {
                partial_text: "正在识别中".to_string(),
                waveform: " ▃▅▇ ".to_string(),
                display_text: None,
                is_listening: true,
                visual_effect: effect,
            }),
        };
        assert!(
            renderer.render(&state).is_some(),
            "语音视觉风格 {:?} 应成功渲染",
            effect
        );
    }
}
