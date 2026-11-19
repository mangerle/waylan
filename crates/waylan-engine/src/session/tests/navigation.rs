use super::super::*;
use waylan_algo::table::LexiconTable;
use waylan_algo::types::AlgoConfig;
use waylan_core::CandidateExpansion;

#[test]
fn test_ime_core_session_grid_navigation_and_selection() {
    let mut table = LexiconTable::with_sample_data();
    for i in 0..20 {
        table.insert("ni", format!("字{}", i), 10_000 - i * 10);
    }
    let mut session = ImeCoreSession::new(AlgoEngine::new(table, AlgoConfig::default()));
    session.handle_key_press(49, 1000, None); // 'n'
    session.handle_key_press(23, 1001, None); // 'i'

    // 展开大网格面板
    session.expand_grid();
    assert!(session.is_grid_expanded());
    assert_eq!(session.grid_selected_index(), 0);

    // 1. 按 Down 键：向下移动一行 (第 2 行第 1 个词，索引 6)
    let act_down = session.handle_key_press(KEY_DOWN, 1002, None);
    assert_eq!(act_down, ImeOutputAction::Handled { update_ui: true });
    assert_eq!(session.grid_selected_index(), 6);

    // 2. 按 Right 键：向右移动一个词 (索引 7)
    let act_right = session.handle_key_press(KEY_RIGHT, 1003, None);
    assert_eq!(act_right, ImeOutputAction::Handled { update_ui: true });
    assert_eq!(session.grid_selected_index(), 7);

    // 3. 当前处于第 2 行，按数字 2 键：选取第 2 行第 2 个词 (索引 1 * 6 + 1 = 7)
    let all_cands = session.engine.all_candidates();
    let expected_text = all_cands[7].text.clone();
    let act_num = session.handle_key_press(KEY_2, 1004, None);
    assert_eq!(act_num, ImeOutputAction::Commit(expected_text));
    assert!(!session.is_grid_expanded(), "选词后应自动折叠大网格");
    assert_eq!(session.grid_selected_index(), 0);

    // 4. 测试 ESC 折叠大网格
    session.handle_key_press(49, 1005, None);
    session.handle_key_press(23, 1006, None);
    session.expand_grid();
    let act_esc = session.handle_key_press(KEY_ESC, 1007, None);
    assert_eq!(act_esc, ImeOutputAction::Handled { update_ui: true });
    assert!(!session.is_grid_expanded());
    assert!(!session.engine.raw_input().is_empty());
}

#[test]
fn test_ime_core_session_expanded_card_and_reset_context() {
    let mut session = ImeCoreSession::default();
    assert!(!session.is_card_expanded());
    assert!(!session.is_grid_expanded());

    session.expansion = CandidateExpansion::Card { selected_index: 2 };
    assert!(session.is_card_expanded());

    session.expand_grid();
    assert!(session.is_grid_expanded());
    assert!(!session.is_card_expanded());

    session.reset_context();
    assert!(!session.is_card_expanded());
    assert!(!session.is_grid_expanded());
    assert!(session.engine.raw_input().is_empty());
}
