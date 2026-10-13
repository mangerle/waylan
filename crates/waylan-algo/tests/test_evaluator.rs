use chrono::{Local, TimeZone};
use waylan_algo::evaluator::*;

#[test]
fn test_date_evaluator_riqi() {
    let cand = DateTimeEvaluator::try_evaluate("riqi").expect("应匹配日期");
    assert!(cand.text.contains("年"));
    assert!(cand.is_expandable);
    assert_eq!(cand.sub_candidates.len(), 7);
    assert_eq!(cand.text, cand.sub_candidates[0]);
}

#[test]
fn test_date_evaluator_shijian() {
    let cand = DateTimeEvaluator::try_evaluate("shijian").expect("应匹配时间");
    assert!(cand.text.contains(':'));
    assert!(cand.is_expandable);
    assert!(!cand.sub_candidates.is_empty());
    assert_eq!(cand.text, cand.sub_candidates[0]);
}

#[test]
fn test_chinese_date_format() {
    let fake = Local.with_ymd_and_hms(2026, 10, 9, 16, 3, 26).unwrap();
    let subs = DateTimeEvaluator::format_date_formats(fake);
    assert_eq!(subs[0], "2026年10月9日");
    assert_eq!(subs[1], "20261009");
    assert_eq!(subs[2], "2026-10-09");
    assert_eq!(subs[3], "2026/10/09");
    assert_eq!(subs[4], "2026.10.09");
    assert_eq!(subs[5], "二〇二六年十月九日");
    assert_eq!(subs[6], "丙午[马]年八月廿九");
}

#[test]
fn test_datetime_formats() {
    let fake = Local.with_ymd_and_hms(2026, 10, 9, 16, 3, 26).unwrap();
    let subs = DateTimeEvaluator::format_datetime_formats(fake);
    assert_eq!(subs[0], "2026年10月9日16:03:26");
    assert_eq!(subs[1], "2026-10-09 16:03:26");
    assert_eq!(subs[2], "20261009160326");
    assert_eq!(subs[3], "16:03:26");
}

#[test]
fn test_financial_evaluator() {
    // 1. 常见带角分金额
    let cands = FinancialEvaluator::try_evaluate("12345.67").expect("应解析金额");
    assert_eq!(cands[0].text, "壹万贰仟叁佰肆拾伍元陆角柒分");
    assert_eq!(cands[0].comment.as_deref(), Some("人民币大写"));
    assert_eq!(cands[1].text, "壹万贰仟叁佰肆拾伍点陆柒");
    assert_eq!(cands[2].text, "12,345.67");

    // 2. 整数金额（带整）
    let cands_int = FinancialEvaluator::try_evaluate("1005").expect("应解析整数");
    assert_eq!(cands_int[0].text, "壹仟零伍元整");
    assert_eq!(cands_int[1].text, "壹仟零伍");

    // 3. 亿级金额与零处理
    let cands_billion = FinancialEvaluator::try_evaluate("100000000").expect("应解析亿级");
    assert_eq!(cands_billion[0].text, "壹亿元整");

    // 4. 纯角分小数
    let cands_frac = FinancialEvaluator::try_evaluate("0.5").expect("应解析角");
    assert_eq!(cands_frac[0].text, "伍角整");
    let cands_fen = FinancialEvaluator::try_evaluate("0.05").expect("应解析分");
    assert_eq!(cands_fen[0].text, "伍分");

    // 5. 零元整
    let cands_zero = FinancialEvaluator::try_evaluate("0").expect("应解析零");
    assert_eq!(cands_zero[0].text, "零元整");
}
