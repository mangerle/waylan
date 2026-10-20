use crate::prism::PrismEngine;
use crate::syllables;
use crate::table::LexiconTable;
use crate::types::{self, AlgoCandidate, CandidateTier};
use waylan_core::config::{FuzzyPinyinRules, TypoCorrectionRules};

/// 物理键盘空间邻键误触与相邻字符倒置容错纠错模块
pub struct SpatialTypoCorrector;

impl SpatialTypoCorrector {
    /// 生成基于物理键盘邻键误触与相邻倒置的次位容错候选
    ///
    /// # 设计原理
    /// - **空间距离判定**：基于 QWERTY 键盘欧几里得距离，限制在 1.25 键距内的邻键替换。
    /// - **相邻字母倒置**：针对拼音快速击键常见的前后顺序倒置（如 `teh` -> `the`, `cehng` -> `cheng`）。
    /// - **规则门禁**：主开关关闭时完全不生成纠错；方言软纠错分别受模糊音规则控制。
    /// - **排序与截断**：按替换物理代价升序、词频降序综合排序，仅返回前 3 个最优纠错结果。
    pub fn generate_candidates(
        raw_input: &str,
        table: &LexiconTable,
        fuzzy: &FuzzyPinyinRules,
        typo_rules: &TypoCorrectionRules,
    ) -> Vec<AlgoCandidate> {
        if !typo_rules.enable {
            return Vec::new();
        }
        let n = raw_input.len();
        // 限制拼写纠错在合理词长（3..=16）内启用，覆盖 1~4 音节常用多字词容错
        if !(3..=16).contains(&n) || !raw_input.is_ascii() {
            return Vec::new();
        }

        let mut candidates: Vec<(AlgoCandidate, u32)> = Vec::with_capacity(16);

        // 1. 物理二维键盘空间邻键矩阵化替换
        Self::collect_spatial_matrix_candidates(raw_input, table, &mut candidates);

        // 2. 邻近字母快速倒置
        Self::collect_adjacent_swap_candidates(raw_input, table, &mut candidates);

        // 3. 受模糊音与纠错规则控制的方言软约束纠错
        Self::collect_rule_soft_corrections(raw_input, table, fuzzy, typo_rules, &mut candidates);

        // 4. 排序与去重：优先代价低，次优先词频高
        candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.score.cmp(&a.0.score)));

        let mut deduped = Vec::with_capacity(3);
        for (cand, _) in candidates {
            if !deduped.iter().any(|d: &AlgoCandidate| d.text == cand.text) {
                deduped.push(cand);
                if deduped.len() >= 3 {
                    break;
                }
            }
        }

        deduped
    }

    /// 遍历输入序列并基于 QWERTY 键盘二维物理邻键矩阵查表生成替换候选
    fn collect_spatial_matrix_candidates(
        raw_input: &str,
        table: &LexiconTable,
        candidates: &mut Vec<(AlgoCandidate, u32)>,
    ) {
        let n = raw_input.len();
        let mut buf = [0u8; 32];
        buf[..n].copy_from_slice(raw_input.as_bytes());

        for i in 0..n {
            let orig_b = buf[i];
            if !orig_b.is_ascii_lowercase() {
                continue;
            }
            let orig_c = orig_b as char;

            // 基于预计算静态矩阵的紧邻键表极速遍历，彻底消除浮点计算与冗余循环
            for &(alt_c, cost_u8) in syllables::get_qwerty_neighbors(orig_c) {
                let cost = cost_u8 as u32;
                buf[i] = alt_c as u8;
                if let Ok(mutated) = std::str::from_utf8(&buf[..n]) {
                    Self::try_push_valid_candidate(mutated, cost, table, candidates);
                }
                buf[i] = orig_b;
            }
        }
    }

    /// 遍历输入序列生成相邻按键倒置候选（如 `cehng` -> `cheng`）
    fn collect_adjacent_swap_candidates(
        raw_input: &str,
        table: &LexiconTable,
        candidates: &mut Vec<(AlgoCandidate, u32)>,
    ) {
        let n = raw_input.len();
        let mut buf = [0u8; 32];
        buf[..n].copy_from_slice(raw_input.as_bytes());

        for i in 0..n.saturating_sub(1) {
            buf.swap(i, i + 1);
            if let Ok(swapped) = std::str::from_utf8(&buf[..n]) {
                Self::try_push_valid_candidate(swapped, 16, table, candidates);
            }
            buf.swap(i, i + 1);
        }
    }

    /// 根据模糊音与拼音纠错规则生成方言软约束纠错
    ///
    /// 处理流程依次为：通用 gn/mg 误输归一、平翘舌软约束互换、鼻音边音混淆、前后鼻音混淆，
    /// 各阶段产出的变异输入统一按代价权重推入候选列表。
    fn collect_rule_soft_corrections(
        raw_input: &str,
        table: &LexiconTable,
        fuzzy: &FuzzyPinyinRules,
        typo_rules: &TypoCorrectionRules,
        candidates: &mut Vec<(AlgoCandidate, u32)>,
    ) {
        let mut mutated_inputs = Vec::with_capacity(8);

        // 1. 通用 gn/mg 误输替换为 ng
        Self::collect_gn_mg_corrections(raw_input, typo_rules, &mut mutated_inputs);

        // 2. 平翘舌音软约束互换，分别受对应模糊音开关控制
        Self::collect_retroflex_corrections(raw_input, fuzzy, &mut mutated_inputs);

        // 3. 鼻音边音软约束混淆
        Self::collect_lateral_nasal_corrections(raw_input, fuzzy, &mut mutated_inputs);

        // 4. 前后鼻音软约束混淆，分别受对应模糊音开关控制
        Self::collect_back_front_nasal_corrections(raw_input, fuzzy, &mut mutated_inputs);

        for (mutated, cost) in mutated_inputs {
            if mutated != raw_input {
                Self::try_push_valid_candidate(&mutated, cost, table, candidates);
            }
        }
    }

    /// 收集通用 gn/mg 误输归一为 ng 的候选变异输入
    fn collect_gn_mg_corrections(
        raw_input: &str,
        typo_rules: &TypoCorrectionRules,
        mutated_inputs: &mut Vec<(String, u32)>,
    ) {
        if typo_rules.gn_ng && raw_input.contains("gn") {
            mutated_inputs.push((raw_input.replace("gn", "ng"), 18));
        }
        if typo_rules.mg_ng && raw_input.contains("mg") {
            mutated_inputs.push((raw_input.replace("mg", "ng"), 18));
        }
    }

    /// 收集平舌与卷舌声母软约束互换（组合纠错与单对互换）产生的候选变异输入
    fn collect_retroflex_corrections(
        raw_input: &str,
        fuzzy: &FuzzyPinyinRules,
        mutated_inputs: &mut Vec<(String, u32)>,
    ) {
        let mut retroflex_pairs = Vec::with_capacity(3);
        if fuzzy.z_zh {
            retroflex_pairs.push(("zh", "z"));
        }
        if fuzzy.c_ch {
            retroflex_pairs.push(("ch", "c"));
        }
        if fuzzy.s_sh {
            retroflex_pairs.push(("sh", "s"));
        }

        // 组合纠错：同时应用所有已启用的平舌转卷舌规则（如 cuanze => chuanzhe）
        let mut combined = raw_input.to_string();
        let mut combined_changed = false;
        for &(retro, dental) in &retroflex_pairs {
            if raw_input.contains(dental) {
                let (converted, changed) = Self::convert_dental_initial(&combined, dental, retro);
                combined = converted;
                combined_changed |= changed;
            }
        }
        if combined_changed {
            mutated_inputs.push((combined, 22));
        }

        // 单对平翘舌独立互换
        for (retro, dental) in retroflex_pairs {
            if raw_input.contains(retro) {
                mutated_inputs.push((raw_input.replace(retro, dental), 22));
            } else if raw_input.contains(dental) {
                let (converted, changed) = Self::convert_dental_initial(raw_input, dental, retro);
                if changed {
                    mutated_inputs.push((converted, 22));
                }
            }
        }
    }

    /// 收集鼻音 l/n 软约束混淆产生的候选变异输入
    fn collect_lateral_nasal_corrections(
        raw_input: &str,
        fuzzy: &FuzzyPinyinRules,
        mutated_inputs: &mut Vec<(String, u32)>,
    ) {
        if fuzzy.l_n {
            if let Some(rest) = raw_input.strip_prefix('l') {
                mutated_inputs.push((format!("n{}", rest), 24));
            } else if let Some(rest) = raw_input.strip_prefix('n') {
                mutated_inputs.push((format!("l{}", rest), 24));
            }
        }
    }

    /// 收集前后鼻音软约束混淆（ang/an、eng/en、ing/in）产生的候选变异输入
    fn collect_back_front_nasal_corrections(
        raw_input: &str,
        fuzzy: &FuzzyPinyinRules,
        mutated_inputs: &mut Vec<(String, u32)>,
    ) {
        let mut nasal_pairs = Vec::with_capacity(3);
        if fuzzy.an_ang {
            nasal_pairs.push(("ang", "an"));
        }
        if fuzzy.en_eng {
            nasal_pairs.push(("eng", "en"));
        }
        if fuzzy.in_ing {
            nasal_pairs.push(("ing", "in"));
        }
        for (back, front) in nasal_pairs {
            if raw_input.contains(back) {
                mutated_inputs.push((raw_input.replace(back, front), 24));
            } else if raw_input.contains(front) {
                mutated_inputs.push((raw_input.replace(front, back), 24));
            }
        }
    }

    /// 将字符串中未被 h 跟随的平舌声母替换为卷舌声母，返回新字符串与是否发生替换
    fn convert_dental_initial(input: &str, dental: &str, retroflex: &str) -> (String, bool) {
        let bytes = input.as_bytes();
        let target_b = dental.as_bytes()[0];
        let mut out = String::with_capacity(input.len() + 4);
        let mut replaced = false;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == target_b && (i + 1 == bytes.len() || bytes[i + 1] != b'h') {
                out.push_str(retroflex);
                replaced = true;
            } else {
                out.push(bytes[i] as char);
            }
            i += 1;
        }
        (out, replaced)
    }

    /// 尝试校验变异字符串是否构成单音节或可合法切分，命中词条则推入候选列表
    fn try_push_valid_candidate(
        mutated: &str,
        cost: u32,
        table: &LexiconTable,
        candidates: &mut Vec<(AlgoCandidate, u32)>,
    ) {
        if syllables::is_valid_syllable(mutated) {
            if let Some(entries) = table.get_raw_entries(mutated) {
                for (text, raw_score) in entries.iter().take(2) {
                    let mut cand = AlgoCandidate::new(text, mutated, *raw_score);
                    cand.tier = CandidateTier::TypoCorrection;
                    cand.comment = Some(format!("[纠错: {}]", mutated));
                    candidates.push((cand, cost));
                }
            }
        } else if syllables::can_segment_into_syllables(mutated) {
            let paths = PrismEngine::build_paths(mutated, &types::FuzzyConfig::default());
            if let Some(best) = paths.first() {
                if best.remaining.is_empty() && !best.syllables.is_empty() {
                    let key = best.to_key_string();
                    if let Some(entries) = table.get_raw_entries(&key) {
                        for (text, raw_score) in entries.iter().take(2) {
                            let mut cand = AlgoCandidate::new(text, &key, *raw_score);
                            cand.tier = CandidateTier::TypoCorrection;
                            cand.comment = Some(format!("[纠错: {}]", key));
                            candidates.push((cand, cost));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate(input: &str, table: &LexiconTable) -> Vec<AlgoCandidate> {
        SpatialTypoCorrector::generate_candidates(
            input,
            table,
            &FuzzyPinyinRules::all_enabled(),
            &TypoCorrectionRules::default(),
        )
    }

    #[test]
    fn test_spatial_typo_corrector_pruning_and_recovery() {
        let mut table = LexiconTable::new();
        table.insert("ni'hao", "你好", 50_000);
        table.insert("cheng", "成", 40_000);

        // 邻键替换纠错：nihso (s->a) 召回 你好
        let cands_sub = generate("nihso", &table);
        assert!(!cands_sub.is_empty());
        assert_eq!(cands_sub[0].text, "你好");

        // 相邻倒置纠错：cehng (eh->he) 召回 成
        let cands_swap = generate("cehng", &table);
        assert!(!cands_swap.is_empty());
        assert_eq!(cands_swap[0].text, "成");

        // 超过 8 字节的多音节长词邻键纠错：zhongguorwn (11字节, w->e) 召回 中国人 (zhong'guo'ren)
        table.insert("zhong'guo'ren", "中国人", 60_000);
        let cands_long = generate("zhongguorwn", &table);
        assert!(!cands_long.is_empty());
        assert_eq!(cands_long[0].text, "中国人");
    }

    #[test]
    fn test_qwerty_matrix_o_and_c_neighbor_recall() {
        let mut table = LexiconTable::new();
        table.insert("pin'yin", "拼音", 50_000);
        table.insert("kai'xin", "开心", 40_000);

        // 误将 p 打成 o (o 误触 p)
        let cands_o = generate("oinyin", &table);
        assert!(
            !cands_o.is_empty(),
            "oinyin 应通过 o->p 矩阵邻键纠错召回 拼音"
        );
        assert_eq!(cands_o[0].text, "拼音");

        // 误将 c 打成 v (v 误触 c)
        table.insert("cheng'gong", "成功", 60_000);
        let cands_v = generate("vhenggong", &table);
        assert!(
            !cands_v.is_empty(),
            "vhenggong 应通过 v->c 矩阵邻键纠错召回 成功"
        );
        assert_eq!(cands_v[0].text, "成功");
    }

    #[test]
    fn test_dialect_soft_corrections_gn_and_retroflex() {
        let mut table = LexiconTable::new();
        table.insert("liang'hao", "良好", 50_000);
        table.insert("chuan'zhe", "穿着", 45_000);

        // 通用 gn 误输纠错: liagnhao -> lianghao
        let cands_gn = generate("liagnhao", &table);
        assert!(!cands_gn.is_empty(), "liagnhao 应纠错为良好");
        assert_eq!(cands_gn[0].text, "良好");

        // 平舌卷舌音长词自适应软约束纠错: cuanze -> chuanzhe
        let cands_retro = generate("cuanze", &table);
        assert!(!cands_retro.is_empty(), "cuanze 应自适应软约束纠错召回穿着");
        assert_eq!(cands_retro[0].text, "穿着");
    }

    #[test]
    fn test_typo_rules_master_and_fuzzy_switches() {
        let mut table = LexiconTable::new();
        table.insert("chuan'zhe", "穿着", 45_000);

        let disabled_master = TypoCorrectionRules {
            enable: false,
            ..Default::default()
        };
        assert!(SpatialTypoCorrector::generate_candidates(
            "cuanze",
            &table,
            &FuzzyPinyinRules::all_enabled(),
            &disabled_master,
        )
        .is_empty());

        let fuzzy_disabled = FuzzyPinyinRules::default();
        assert!(SpatialTypoCorrector::generate_candidates(
            "cuanze",
            &table,
            &fuzzy_disabled,
            &TypoCorrectionRules::default(),
        )
        .is_empty());
    }
}
