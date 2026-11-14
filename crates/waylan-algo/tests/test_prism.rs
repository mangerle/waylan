use waylan_algo::decoder::{EdgeKind, LatticeOptions, PrismFuzzy, SyllableEdge, SyllableLattice};
use waylan_algo::types::FuzzyConfig;

fn collect_lattice_paths(lattice: &SyllableLattice) -> Vec<String> {
    fn dfs(lattice: &SyllableLattice, pos: usize, acc: &mut Vec<String>, out: &mut Vec<String>) {
        if out.len() >= 128 {
            return;
        }
        let next = lattice.skip_separators(pos);
        if next >= lattice.len {
            out.push(acc.join("'"));
            return;
        }
        for edge in &lattice.edges_from[next] {
            let prev_len = acc.len();
            acc.extend(edge.syllables.iter().cloned());
            dfs(lattice, edge.end, acc, out);
            acc.truncate(prev_len);
        }
    }
    let mut out = Vec::new();
    let mut acc = Vec::new();
    dfs(lattice, 0, &mut acc, &mut out);
    out
}

fn has_edge_matching(
    lattice: &SyllableLattice,
    pos: usize,
    pred: impl Fn(&SyllableEdge) -> bool,
) -> bool {
    lattice
        .edges_from
        .get(pos)
        .is_some_and(|edges| edges.iter().any(pred))
}

#[test]
fn test_dag_ambiguity_xian() {
    let lattice = SyllableLattice::build("xian", &LatticeOptions::default());
    let paths = collect_lattice_paths(&lattice);
    assert!(paths.contains(&"xian".to_string()));
    assert!(paths.contains(&"xi'an".to_string()));
}

#[test]
fn test_explicit_split_quote() {
    let lattice = SyllableLattice::build("xi'an", &LatticeOptions::default());
    assert_eq!(lattice.best_formatted_preedit(), "xi'an");
    assert!(!has_edge_matching(&lattice, 0, |e| e.syllables == vec!["xian"]));

    let lattice_prefix = SyllableLattice::build("xi'a", &LatticeOptions::default());
    let paths = collect_lattice_paths(&lattice_prefix);
    assert!(paths.contains(&"xi'a".to_string()));
}

#[test]
fn test_partial_remaining() {
    let lattice = SyllableLattice::build("shux", &LatticeOptions::default());
    assert!(has_edge_matching(&lattice, 0, |e| e.syllables
        == vec!["shu"]
        && e.end == 3));
    assert!(has_edge_matching(&lattice, 3, |e| {
        e.kind == EdgeKind::PartialTail && e.syllables == vec!["x"] && e.end == 4
    }));
}

#[test]
fn test_strict_raw_initials_segmentation() {
    let opts = LatticeOptions {
        enable_mixed_jianpin: true,
        ..Default::default()
    };
    let lattice = SyllableLattice::build("qwdwwqswsqw", &opts);
    assert_eq!(lattice.best_formatted_preedit(), "q'w'd'w'w'q's'w's'q'w");
}

#[test]
fn test_zero_initial_borrowing_split_cost() {
    let lattice = SyllableLattice::build("jintianxingqitian", &LatticeOptions::default());
    assert_eq!(lattice.best_formatted_preedit(), "jin'tian'xing'qi'tian");
}

#[test]
fn test_typo_adjacent_transpose_and_spatial_graph() {
    let opts = LatticeOptions {
        enable_typo_edges: true,
        ..Default::default()
    };
    let lat_cehng = SyllableLattice::build("cehng", &opts);
    assert!(has_edge_matching(&lat_cehng, 0, |e| {
        e.syllables == vec!["cheng"] && e.typo_count == 1
    }));

    let lat_nihso = SyllableLattice::build("nihso", &opts);
    let paths = collect_lattice_paths(&lat_nihso);
    assert!(paths.contains(&"ni'hao".to_string()));
}

#[test]
fn test_fuzzy_nine_rules_expansion() {
    let fuzzy = FuzzyConfig::all_enabled();
    let zong_alts = PrismFuzzy::expand_fuzzy_syllables("zong", &fuzzy);
    assert!(zong_alts.contains(&"zhong".to_string()));

    let lu_alts = PrismFuzzy::expand_fuzzy_syllables("lu", &fuzzy);
    assert!(lu_alts.contains(&"nu".to_string()));
    assert!(lu_alts.contains(&"ru".to_string()));

    let fa_alts = PrismFuzzy::expand_fuzzy_syllables("fa", &fuzzy);
    assert!(fa_alts.contains(&"ha".to_string()));

    let lan_alts = PrismFuzzy::expand_fuzzy_syllables("lan", &fuzzy);
    assert!(lan_alts.contains(&"lang".to_string()));
    assert!(lan_alts.contains(&"nan".to_string()));
    assert!(lan_alts.contains(&"nang".to_string()));

    let sen_alts = PrismFuzzy::expand_fuzzy_syllables("sen", &fuzzy);
    assert!(sen_alts.contains(&"seng".to_string()));
    assert!(sen_alts.contains(&"shen".to_string()));
    assert!(sen_alts.contains(&"sheng".to_string()));
}

#[test]
fn test_fuzzy_engine_lookup_zongguo() {
    let config = waylan_algo::types::AlgoConfig {
        fuzzy: FuzzyConfig::all_enabled(),
        ..Default::default()
    };
    let mut engine = waylan_algo::AlgoEngine::with_sample_data(config);
    for ch in "zongguo".chars() {
        engine.push_char(ch);
    }
    let cands = engine.candidates();
    assert!(!cands.is_empty(), "模糊音开启时输入 zongguo 应当召回候选词");
    assert!(
        cands.iter().any(|c| c.text == "中国"),
        "zongguo 应当通过模糊音规则召回“中国”"
    );
}

#[test]
fn test_habitual_alias_paths() {
    let lat_en = SyllableLattice::build("en", &LatticeOptions::default());
    let paths_en = collect_lattice_paths(&lat_en);
    assert!(paths_en.contains(&"en".to_string()));
    assert!(paths_en.contains(&"n".to_string()));
    assert!(paths_en.contains(&"ng".to_string()));

    let lat_enen = SyllableLattice::build("enen", &LatticeOptions::default());
    let paths_enen = collect_lattice_paths(&lat_enen);
    assert!(paths_enen.contains(&"ng'ng".to_string()));

    let lat_enhao = SyllableLattice::build("enhao", &LatticeOptions::default());
    let paths_enhao = collect_lattice_paths(&lat_enhao);
    assert!(paths_enhao.contains(&"ng'hao".to_string()));
}
