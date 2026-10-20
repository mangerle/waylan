use super::*;
use crate::types::FuzzyConfig;
use waylan_core::config::TypoCorrectionRules;

#[test]
fn test_non_ascii_input_safe_degradation() {
    let fuzzy = FuzzyConfig {
        z_zh: true,
        an_ang: true,
        ..Default::default()
    };

    // 纯中文非受保护输入应安全退化，不触发 UTF-8 字符边界切片 Panic
    let paths = PrismEngine::build_paths("你好世界", &fuzzy);
    assert_eq!(paths.len(), 1);
    assert!(paths[0].syllables.is_empty());
    assert_eq!(paths[0].remaining, "你好世界");

    // 混合拼音与中文的多字节输入应安全退化，不触发 Panic
    let mixed_paths = PrismEngine::build_paths("pin音测试", &fuzzy);
    assert!(!mixed_paths.is_empty());
}

#[test]
fn test_extended_transpose_and_cross_boundary_typo() {
    let fuzzy = FuzzyConfig::default();

    // 1. 长音节尾部倒置 (idx 4, 5): zhuagn -> zhuang
    let zhuang_paths = PrismEngine::build_paths("zhuagn", &fuzzy);
    assert!(
        zhuang_paths
            .iter()
            .any(|p| p.to_key_string() == "zhuang" && p.is_typo()),
        "应支持长音节尾部 (4,5) 倒置纠错 zhuagn -> zhuang"
    );

    // 2. 跨音节边界首尾倒置: nhiao -> ni'hao, shhiou -> shi'hou
    let nhiao_paths = PrismEngine::build_paths("nhiao", &fuzzy);
    assert!(
        nhiao_paths
            .iter()
            .any(|p| p.to_key_string() == "ni'hao" && p.is_typo()),
        "应支持跨音节边界倒置 nhiao -> ni'hao"
    );

    let shhiou_paths = PrismEngine::build_paths("shhiou", &fuzzy);
    assert!(
        shhiou_paths
            .iter()
            .any(|p| p.to_key_string() == "shi'hou" && p.is_typo()),
        "应支持跨音节边界倒置 shhiou -> shi'hou"
    );

    // 3. 第二音节开头倒置且被单字母元音伪装: shiohu -> shi'hou
    let shiohu_paths = PrismEngine::build_paths("shiohu", &fuzzy);
    assert!(
        shiohu_paths
            .iter()
            .any(|p| p.to_key_string() == "shi'hou" && p.is_typo()),
        "应支持 shiohu -> shi'hou 倒置纠错"
    );
}

#[test]
fn test_explicit_quote_multisyllable_part_split() {
    let fuzzy = FuzzyConfig::default();
    let paths = PrismEngine::build_paths("xi'anhui", &fuzzy);
    assert!(!paths.is_empty());
    assert_eq!(
        paths[0].syllables,
        vec!["xi".to_string(), "an".to_string(), "hui".to_string()]
    );
    assert!(paths[0].remaining.is_empty());
}

#[test]
fn test_dedup_best_paths_retains_optimal() {
    let mut paths = vec![
        SegmentPath {
            syllables: vec!["ni".to_string(), "hao".to_string()],
            remaining: String::new(),
            cost: 36,
            origin: PathOrigin::SpatialTypo,
        },
        SegmentPath {
            syllables: vec!["ni".to_string(), "hao".to_string()],
            remaining: "a".to_string(),
            cost: 25,
            origin: PathOrigin::Exact,
        },
        SegmentPath {
            syllables: vec!["ni".to_string(), "hao".to_string()],
            remaining: String::new(),
            cost: 20,
            origin: PathOrigin::Exact,
        },
    ];
    PrismEngine::dedup_best_paths(&mut paths);
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0].remaining, "");
    assert_eq!(paths[0].cost, 20);
    assert!(!paths[0].is_typo());
    assert_eq!(paths[1].remaining, "a");
}

#[test]
fn test_typo_rules_master_switch() {
    let fuzzy = FuzzyConfig::default();

    let enabled = TypoCorrectionRules::default();
    let paths = PrismEngine::build_paths_with_options("zhuagn", &fuzzy, &enabled, None);
    assert!(
        paths.iter().any(|p| p.is_typo()),
        "开启纠错规则时应生成容错路径"
    );

    let master_disabled = TypoCorrectionRules {
        enable: false,
        ..Default::default()
    };
    let paths = PrismEngine::build_paths_with_options("zhuagn", &fuzzy, &master_disabled, None);
    assert!(
        paths.iter().all(|p| !p.is_typo()),
        "纠错主开关关闭后不得生成任何容错路径"
    );
}

#[test]
fn test_normal_flow_still_applies_rule_normalization() {
    let fuzzy = FuzzyConfig::default();
    let rules = TypoCorrectionRules::default();

    // `niou` 可被精确切成 `ni'ou`（属正常击键流），但 iou->iu 归一仍须召回单音节 `niu`
    let paths = PrismEngine::build_paths_with_options("niou", &fuzzy, &rules, None);
    let normalized = paths
        .iter()
        .find(|p| p.to_key_string() == "niu")
        .expect("正常击键流下应产出 iou->iu 归一路径");
    assert_eq!(normalized.origin, PathOrigin::RuleNormalized);
    assert_eq!(
        paths.first().expect("路径列表不应为空").origin,
        PathOrigin::Exact,
        "严格切分路径必须仍排在首位"
    );
}

#[test]
fn test_rule_switch_off_disables_normal_flow_normalization() {
    let fuzzy = FuzzyConfig::default();
    let rules = TypoCorrectionRules {
        iou_iu: false,
        ..Default::default()
    };
    let paths = PrismEngine::build_paths_with_options("niou", &fuzzy, &rules, None);
    assert!(
        paths.iter().all(|p| p.origin == PathOrigin::Exact),
        "关闭 iou->iu 细分规则后，正常流不得残留任何归一或猜测路径"
    );
}

#[test]
fn test_normal_flow_never_generates_spatial_guess() {
    let fuzzy = FuzzyConfig::default();
    let rules = TypoCorrectionRules::default();

    // 正常全拼击键流只允许确定性归一，物理邻键猜测不得介入
    for input in ["women", "nihao", "jintian", "lv", "niou", "zhongguo"] {
        let paths = PrismEngine::build_paths_with_options(input, &fuzzy, &rules, None);
        assert!(
            paths.iter().all(|p| p.origin != PathOrigin::SpatialTypo),
            "输入 {} 属正常击键流，不应产生概率性邻键路径",
            input
        );
    }
}

#[test]
fn test_broken_flow_enables_full_spatial_exploration() {
    let fuzzy = FuzzyConfig::default();
    let rules = TypoCorrectionRules::default();

    // `nihso` 无法完整切分（非正常流），仍应走全量容错并标记为邻键档
    let paths = PrismEngine::build_paths_with_options("nihso", &fuzzy, &rules, None);
    assert!(
        paths
            .iter()
            .any(|p| p.to_key_string() == "ni'hao" && p.origin == PathOrigin::SpatialTypo),
        "断流输入应启用全量容错虚边并归入邻键猜测档"
    );
}
