use waylan_algo::table::LexiconTable;

#[test]
fn test_lookup_length_boost() {
    let table = LexiconTable::with_sample_data();
    let candidates = table.lookup("wo'men");
    assert!(!candidates.is_empty());
    assert_eq!(candidates[0].text, "我们");
    assert!(candidates[0].score >= 600_000);
}

#[test]
fn test_lookup_xian_vs_xi_an() {
    let table = LexiconTable::with_sample_data();
    let c_xian = table.lookup("xian");
    let c_xi_an = table.lookup("xi'an");

    assert!(c_xian.iter().any(|c| c.text == "先"));
    assert!(c_xi_an.iter().any(|c| c.text == "西安"));
}
