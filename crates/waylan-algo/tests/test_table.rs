use waylan_algo::table::{calculate_length_boost, extract_raw_score, LexiconTable};
use waylan_algo::LENGTH_BOOST_MULTIPLIER;

#[test]
fn test_lookup_length_boost() {
    let table = LexiconTable::with_sample_data();
    let candidates = table.lookup("wo'men");
    assert!(!candidates.is_empty());
    assert_eq!(candidates[0].text, "我们");
    assert!(candidates[0].score >= LENGTH_BOOST_MULTIPLIER);
}

#[test]
fn test_length_boost_and_raw_score_calculation() {
    assert_eq!(calculate_length_boost(0), 0);
    assert_eq!(calculate_length_boost(1), 0);
    assert_eq!(calculate_length_boost(2), LENGTH_BOOST_MULTIPLIER);
    assert_eq!(calculate_length_boost(3), LENGTH_BOOST_MULTIPLIER * 2);

    let raw = 12345;
    let boosted = raw + calculate_length_boost(4);
    assert_eq!(extract_raw_score(boosted), raw);
}

#[test]
fn test_lookup_xian_vs_xi_an() {
    let table = LexiconTable::with_sample_data();
    let c_xian = table.lookup("xian");
    let c_xi_an = table.lookup("xi'an");

    assert!(c_xian.iter().any(|c| c.text == "先"));
    assert!(c_xi_an.iter().any(|c| c.text == "西安"));
}
