//! 候选词与候选框状态机单元测试

use super::entity::*;
use super::format::*;
use super::state::*;
use super::view::*;
use crate::config::CandidateIndexStyle;

#[test]
fn test_candidate_creation() {
    let c = Candidate::new("你好", "ni'hao", 100);
    assert_eq!(c.text, "你好");
    assert_eq!(c.pinyin, "ni'hao");
    assert_eq!(c.score, 100);
    assert!(!c.is_expandable);
    assert!(c.sub_candidates.is_empty());
    assert_eq!(c.comment, None);

    let c_expanded = Candidate::new("今天", "jin'tian", 90)
        .with_sub_candidates(vec!["2026-10-18".to_string(), "2026年10月18日".to_string()])
        .with_comment("日期");
    assert!(c_expanded.is_expandable);
    assert_eq!(c_expanded.sub_candidates.len(), 2);
    assert_eq!(c_expanded.comment, Some("日期".to_string()));
}

#[test]
fn test_candidate_box_state_defaults() {
    let state = CandidateBoxState::default();
    assert!(!state.visible);
    assert_eq!(state.preedit, "");
    assert_eq!(state.selected_index, 0);
    assert_eq!(state.expansion, CandidateExpansion::None);
    assert_eq!(state.mode_badge, "");
    assert!(!state.is_grid_expanded());
    assert!(state.grid_candidates.is_empty());
    assert_eq!(state.voice, None);
    assert!(!state.is_voice_active());
    assert!(!state.is_expanded());
    assert_eq!(state.expanded_selected_index(), 0);
}

#[test]
fn test_candidate_box_state_expanded() {
    let mut state = CandidateBoxState::default();
    state.set_expanded(true, 3);
    assert!(state.is_expanded());
    assert_eq!(state.expanded_selected_index(), 3);

    state.set_expanded(false, 0);
    assert!(!state.is_expanded());
    assert_eq!(state.expanded_selected_index(), 0);
}

#[test]
fn test_candidate_view_mode_mutually_exclusive() {
    let mut state = CandidateBoxState::default();
    assert_eq!(state.view_mode(), CandidateViewMode::Hidden);

    // 未激活 visible 时，无论是否有候选均保持 Hidden
    state.candidates.push(Candidate::new("测试", "ce'shi", 100));
    assert_eq!(state.view_mode(), CandidateViewMode::Hidden);

    // 激活 visible 后，包含普通候选时为 Compact
    state.visible = true;
    assert_eq!(state.view_mode(), CandidateViewMode::Compact);

    // 展开二级卡片时为 ExpandedCard
    state.set_expanded(true, 0);
    assert_eq!(state.view_mode(), CandidateViewMode::ExpandedCard);

    // 网格展开时优先为 GridExpanded
    state.set_grid_expanded(true, 0);
    assert_eq!(state.view_mode(), CandidateViewMode::GridExpanded);

    // 激活语音时最高优先级为 VoiceCapsule
    state.voice = Some(VoiceBoxState {
        is_listening: true,
        ..Default::default()
    });
    assert_eq!(state.view_mode(), CandidateViewMode::VoiceCapsule);

    // 关闭 visible 时，即便语音活跃也必须安全隐藏
    state.visible = false;
    assert_eq!(state.view_mode(), CandidateViewMode::Hidden);
}

#[test]
fn test_candidate_box_state_mutual_exclusion() {
    let mut state = CandidateBoxState {
        visible: true,
        ..Default::default()
    };
    state.candidates.push(Candidate::new("测试", "ce'shi", 100));

    // 展开二级卡片
    state.set_expanded(true, 1);
    assert!(state.is_expanded());
    assert!(!state.is_grid_expanded());
    assert_eq!(state.view_mode(), CandidateViewMode::ExpandedCard);
    assert_eq!(
        state.expansion,
        CandidateExpansion::Card { selected_index: 1 }
    );

    // 设置大网格展开，在类型系统层面收敛为互斥枚举
    state.set_grid_expanded(true, 2);
    assert!(state.is_grid_expanded());
    assert!(!state.is_expanded());
    assert_eq!(state.view_mode(), CandidateViewMode::GridExpanded);
    assert_eq!(
        state.expansion,
        CandidateExpansion::Grid { selected_index: 2 }
    );

    // 重新展开二级卡片，大网格必须被物理自动折叠
    state.set_expanded(true, 0);
    assert!(state.is_expanded());
    assert!(!state.is_grid_expanded());
    assert_eq!(state.view_mode(), CandidateViewMode::ExpandedCard);
    assert_eq!(
        state.expansion,
        CandidateExpansion::Card { selected_index: 0 }
    );
}

#[test]
fn test_format_candidate_display_styles() {
    let text = "我们";

    // 现代空白风
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::ModernSpace, 0, text, None),
        "1 我们"
    );
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::ModernSpace, 8, text, None),
        "9 我们"
    );
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::ModernSpace, 9, text, None),
        "0 我们"
    );

    // 经典点号风
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::ClassicDot, 0, text, None),
        "1. 我们"
    );
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::ClassicDot, 9, text, None),
        "0. 我们"
    );

    // 圆圈序号风
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::Circled, 0, text, None),
        "①\u{2009}我们"
    );
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::Circled, 9, text, None),
        "⑩\u{2009}我们"
    );
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::Circled, 10, text, None),
        "•\u{2009}我们"
    );

    // 括号风
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::SquareBracket, 0, text, None),
        "[1] 我们"
    );
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::Parenthesis, 0, text, None),
        "(1) 我们"
    );

    // 极简风
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::MinimalFirstDot, 0, text, None),
        "•\u{2009}我们"
    );
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::MinimalFirstDot, 1, text, None),
        "我们"
    );

    // 无序号
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::None, 0, text, None),
        "我们"
    );
}

#[test]
fn test_format_candidate_custom_keys() {
    let keys = ['a', 's', 'd', 'f', 'g'];
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::ModernSpace, 0, "你好", Some(&keys)),
        "a. 你好"
    );
    assert_eq!(
        format_candidate_display(CandidateIndexStyle::ModernSpace, 1, "世界", Some(&keys)),
        "s. 世界"
    );
}

#[test]
fn test_candidate_comment_kind_and_formatting() {
    // 1. 无注释
    let c_plain = Candidate::new("你好", "ni'hao", 100);
    assert_eq!(c_plain.comment_kind(), None);
    assert!(!c_plain.should_show_comment(true));
    assert_eq!(c_plain.format_display_text(true), "你好");
    assert_eq!(c_plain.format_display_text(false), "你好");

    // 2. 特异性读音圆括号注释 (chang)
    let c_diff = Candidate::new("常", "chang", 90).with_comment("(chang)");
    assert_eq!(c_diff.comment_kind(), Some(CommentKind::PhoneticDifference));
    assert!(c_diff.should_show_comment(true));
    assert!(c_diff.should_show_comment(false));
    assert_eq!(c_diff.format_display_text(true), "常 (chang)");
    assert_eq!(c_diff.format_display_text(false), "常 (chang)");

    // 3. 击键纠错提示（新规范为圆括号纯拼音注音 (ni'hao)，去除非必要汉字；同时兼容旧版 [纠错: ni'hao]）
    let c_typo_new = Candidate::new("你好", "ni'hao", 80)
        .with_tier(CandidateTier::TypoCorrection)
        .with_comment("(ni'hao)");
    assert_eq!(c_typo_new.comment_kind(), Some(CommentKind::TypoCorrection));
    assert!(c_typo_new.should_show_comment(false));
    assert_eq!(c_typo_new.format_display_text(false), "你好 (ni'hao)");

    let c_typo_legacy = Candidate::new("你好", "ni'hao", 80).with_comment("[纠错: ni'hao]");
    assert_eq!(
        c_typo_legacy.comment_kind(),
        Some(CommentKind::TypoCorrection)
    );
    assert!(c_typo_legacy.should_show_comment(false));
    assert_eq!(
        c_typo_legacy.format_display_text(false),
        "你好 [纠错: ni'hao]"
    );

    // 4. 标准普通全拼注音 [ni'hao]
    let c_std = Candidate::new("你好", "ni'hao", 70).with_comment("[ni'hao]");
    assert_eq!(c_std.comment_kind(), Some(CommentKind::StandardPinyin));
    assert!(c_std.should_show_comment(true));
    assert!(!c_std.should_show_comment(false));
    assert_eq!(c_std.format_display_text(true), "你好 [ni'hao]");
    assert_eq!(c_std.format_display_text(false), "你好");

    // 5. 特殊中文模式提示 [混拼] / [习惯音] / 拆字
    let c_mixed = Candidate::new("北京", "bei'jing", 60).with_comment("[混拼]");
    assert_eq!(c_mixed.comment_kind(), Some(CommentKind::SpecialHint));
    assert!(c_mixed.should_show_comment(false));
    assert_eq!(c_mixed.format_display_text(false), "北京 [混拼]");

    let c_custom = Candidate::new("㐷", "u'ma", 50).with_comment("拆字");
    assert_eq!(c_custom.comment_kind(), Some(CommentKind::SpecialHint));
    assert!(c_custom.should_show_comment(false));
    assert_eq!(c_custom.format_display_text(false), "㐷 拆字");
}
