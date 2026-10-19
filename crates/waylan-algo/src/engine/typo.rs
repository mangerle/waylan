use crate::prism::PrismEngine;
use crate::syllables;
use crate::table::LexiconTable;
use crate::types::{self, AlgoCandidate, CandidateTier};

/// 物理键盘空间邻键误触与相邻字符倒置容错纠错模块
pub struct SpatialTypoCorrector;

impl SpatialTypoCorrector {
    /// 生成基于物理键盘邻键误触与相邻倒置的次位容错候选
    ///
    /// # 设计原理
    /// - **空间距离判定**：基于 QWERTY 键盘欧几里得距离，限制在 1.25 键距内的邻键替换。
    /// - **相邻字母倒置**：针对拼音快速击键常见的前后顺序倒置（如 `teh` -> `the`, `cehng` -> `cheng`）。
    /// - **排序与截断**：按替换物理代价升序、词频降序综合排序，仅返回前 3 个最优纠错结果。
    pub fn generate_candidates(raw_input: &str, table: &LexiconTable) -> Vec<AlgoCandidate> {
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

        // 3. 南方方言平翘舌、前后鼻音、鼻边音与通用 gn 误输自适应软约束纠错
        Self::collect_dialect_soft_corrections(raw_input, table, &mut candidates);

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

    /// 南方方言平翘舌、前后鼻音、鼻边音与通用 gn 误输自适应软约束纠错
    fn collect_dialect_soft_corrections(
        raw_input: &str,
        table: &LexiconTable,
        candidates: &mut Vec<(AlgoCandidate, u32)>,
    ) {
        let mut mutated_inputs = Vec::with_capacity(8);

        // 1. 通用 gn 误输替换为 ng (如 liagn => liang, zhegn => zheng)
        if raw_input.contains("gn") {
            mutated_inputs.push((raw_input.replace("gn", "ng"), 18));
        }

        // 2. 平翘舌音软约束互换 (zh <=> z, ch <=> c, sh <=> s)
        // 2.1 全量平舌转卷舌组合（南方长词常见全部漏键入 h：如 cuanze => chuanzhe）
        if let Some(all_retro) = Self::convert_dental_to_retroflex(raw_input) {
            mutated_inputs.push((all_retro, 22));
        }
        // 2.2 全量卷舌转平舌组合
        if let Some(all_dental) = Self::convert_retroflex_to_dental(raw_input) {
            mutated_inputs.push((all_dental, 22));
        }
        // 2.3 单对平翘舌独立互换
        let retroflex_pairs = [("zh", "z"), ("ch", "c"), ("sh", "s")];
        for (retro, dental) in retroflex_pairs {
            if raw_input.contains(retro) {
                mutated_inputs.push((raw_input.replace(retro, dental), 22));
            } else if raw_input.contains(dental) {
                let mut temp = String::with_capacity(raw_input.len() + 2);
                let bytes = raw_input.as_bytes();
                let target_b = dental.as_bytes()[0];
                let mut replaced = false;
                let mut i = 0;
                while i < bytes.len() {
                    if bytes[i] == target_b && (i + 1 == bytes.len() || bytes[i + 1] != b'h') {
                        temp.push_str(retro);
                        replaced = true;
                    } else {
                        temp.push(bytes[i] as char);
                    }
                    i += 1;
                }
                if replaced {
                    mutated_inputs.push((temp, 22));
                }
            }
        }

        // 3. 鼻音边音软约束混淆 (首字母 l <=> n)
        if let Some(rest) = raw_input.strip_prefix('l') {
            mutated_inputs.push((format!("n{}", rest), 24));
        } else if let Some(rest) = raw_input.strip_prefix('n') {
            mutated_inputs.push((format!("l{}", rest), 24));
        }

        // 4. 前后鼻音软约束混淆 (ang <=> an, eng <=> en, ing <=> in)
        let nasal_pairs = [("ang", "an"), ("eng", "en"), ("ing", "in")];
        for (back, front) in nasal_pairs {
            if raw_input.contains(back) {
                mutated_inputs.push((raw_input.replace(back, front), 24));
            } else if raw_input.contains(front) {
                mutated_inputs.push((raw_input.replace(front, back), 24));
            }
        }

        for (mutated, cost) in mutated_inputs {
            if mutated != raw_input {
                Self::try_push_valid_candidate(&mutated, cost, table, candidates);
            }
        }
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

    /// 全量将平舌声母 (z, c, s) 替换为卷舌声母 (zh, ch, sh)
    fn convert_dental_to_retroflex(s: &str) -> Option<String> {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len() + 4);
        let mut i = 0;
        let mut modified = false;
        while i < bytes.len() {
            let b = bytes[i];
            if (b == b'z' || b == b'c' || b == b's')
                && (i + 1 == bytes.len() || bytes[i + 1] != b'h')
            {
                out.push(b as char);
                out.push('h');
                modified = true;
            } else {
                out.push(b as char);
            }
            i += 1;
        }
        if modified {
            Some(out)
        } else {
            None
        }
    }

    /// 全量将卷舌声母 (zh, ch, sh) 替换为平舌声母 (z, c, s)
    fn convert_retroflex_to_dental(s: &str) -> Option<String> {
        if s.contains("zh") || s.contains("ch") || s.contains("sh") {
            Some(s.replace("zh", "z").replace("ch", "c").replace("sh", "s"))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spatial_typo_corrector_pruning_and_recovery() {
        let mut table = LexiconTable::new();
        table.insert("ni'hao", "你好", 50_000);
        table.insert("cheng", "成", 40_000);

        // 邻键替换纠错：nihso (s->a) 召回 你好
        let cands_sub = SpatialTypoCorrector::generate_candidates("nihso", &table);
        assert!(!cands_sub.is_empty());
        assert_eq!(cands_sub[0].text, "你好");

        // 相邻倒置纠错：cehng (eh->he) 召回 成
        let cands_swap = SpatialTypoCorrector::generate_candidates("cehng", &table);
        assert!(!cands_swap.is_empty());
        assert_eq!(cands_swap[0].text, "成");

        // 超过 8 字节的多音节长词邻键纠错：zhongguorwn (11字节, w->e) 召回 中国人 (zhong'guo'ren)
        table.insert("zhong'guo'ren", "中国人", 60_000);
        let cands_long = SpatialTypoCorrector::generate_candidates("zhongguorwn", &table);
        assert!(!cands_long.is_empty());
        assert_eq!(cands_long[0].text, "中国人");
    }

    #[test]
    fn test_qwerty_matrix_o_and_c_neighbor_recall() {
        let mut table = LexiconTable::new();
        table.insert("pin'yin", "拼音", 50_000);
        table.insert("kai'xin", "开心", 40_000);

        // 误将 p 打成 o (o 误触 p)
        let cands_o = SpatialTypoCorrector::generate_candidates("oinyin", &table);
        assert!(
            !cands_o.is_empty(),
            "oinyin 应通过 o->p 矩阵邻键纠错召回 拼音"
        );
        assert_eq!(cands_o[0].text, "拼音");

        // 误将 c 打成 v (v 误触 c)
        table.insert("cheng'gong", "成功", 60_000);
        let cands_v = SpatialTypoCorrector::generate_candidates("vhenggong", &table);
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
        let cands_gn = SpatialTypoCorrector::generate_candidates("liagnhao", &table);
        assert!(!cands_gn.is_empty(), "liagnhao 应纠错为良好");
        assert_eq!(cands_gn[0].text, "良好");

        // 平舌卷舌音长词自适应软约束纠错: cuanze -> chuanzhe
        let cands_retro = SpatialTypoCorrector::generate_candidates("cuanze", &table);
        assert!(!cands_retro.is_empty(), "cuanze 应自适应软约束纠错召回穿着");
        assert_eq!(cands_retro[0].text, "穿着");
    }
}
