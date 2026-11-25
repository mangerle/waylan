//! 标准 QWERTY 键盘物理空间几何模型与加权纠错算法
//!
//! # 设计原理
//! - **实现初衷**：利用物理按键的几何欧式距离加权计算拼写纠错代价，使紧邻按键误触获得更低惩罚。
//! - **核心优势**：通过预计算二维高斯替换代价矩阵与邻键表，在运行期实现纯整数 O(1) 查表。

pub use super::spatial_data::{
    QWERTY_SPATIAL_COST_MATRIX, QWERTY_SPATIAL_NEIGHBORS, ascii_letter_idx, get_qwerty_coord,
};
use super::standard::{STANDARD_SYLLABLES, is_valid_syllable};
use waylan_core::config::TypoCorrectionRules;

/// 获取按键在物理键盘上的紧邻误触按键列表及对应惩罚代价
#[inline]
pub fn get_qwerty_neighbors(c: char) -> &'static [(char, u8)] {
    if let Some(idx) = ascii_letter_idx(c) {
        QWERTY_SPATIAL_NEIGHBORS[idx]
    } else {
        &[]
    }
}

/// 计算两按键在物理键盘上的几何欧氏距离平方（避免热路径调用浮点 `sqrt()`）
#[inline]
pub fn key_spatial_distance_sq(c1: char, c2: char) -> f32 {
    if c1 == c2 {
        return 0.0;
    }
    match (get_qwerty_coord(c1), get_qwerty_coord(c2)) {
        (Some((x1, y1)), Some((x2, y2))) => {
            let dx = x1 - x2;
            let dy = y1 - y2;
            dx * dx + dy * dy
        }
        _ => 25.0,
    }
}

/// 计算两按键在物理键盘上的几何欧氏距离
pub fn key_spatial_distance(c1: char, c2: char) -> f32 {
    key_spatial_distance_sq(c1, c2).sqrt()
}

/// 计算字符替换的高斯几何惩罚代价（单位：惩罚分）
///
/// 算法原理：优先通过 26x26 静态空间矩阵 O(1) 无浮点查表；非 ASCII 降级为动态高斯衰减
#[inline]
pub fn char_substitution_cost(c1: char, c2: char) -> u32 {
    if c1 == c2 {
        return 0;
    }
    if let (Some(i1), Some(i2)) = (ascii_letter_idx(c1), ascii_letter_idx(c2)) {
        return QWERTY_SPATIAL_COST_MATRIX[i1][i2] as u32;
    }
    let d = key_spatial_distance(c1, c2);
    let gaussian_factor = 1.0 - (-(d * d) / 2.0).exp();
    let cost = 10.0 + 35.0 * gaussian_factor;
    cost.round() as u32
}

/// 计算输入切片与目标标准音节之间的物理空间加权编辑距离
///
/// 支持操作：
/// - 替换：根据 QWERTY 空间几何高斯距离加权；
/// - 插入（用户多按键）：代价 26；
/// - 删除（用户漏按键）：代价 26。
pub fn weighted_edit_distance(source: &str, target: &str) -> u32 {
    if source == target {
        return 0;
    }
    if source.is_ascii()
        && target.is_ascii()
        && let Some(distance) = ascii_weighted_edit_distance(source.as_bytes(), target.as_bytes())
    {
        return distance;
    }

    let s_chars: Vec<char> = source.chars().collect();
    let t_chars: Vec<char> = target.chars().collect();
    chars_weighted_edit_distance(&s_chars, &t_chars)
}

/// 计算纯 ASCII 输入的加权编辑距离（长度在栈上小矩阵可承载时走零堆分配快路径，超出范围返回 None）
fn ascii_weighted_edit_distance(s_bytes: &[u8], t_bytes: &[u8]) -> Option<u32> {
    let m = s_bytes.len();
    let n = t_bytes.len();

    if m == 0 {
        return Some((n as u32) * 26);
    }
    if n == 0 {
        return Some((m as u32) * 26);
    }
    if m.abs_diff(n) > 2 {
        return Some(100);
    }
    if m > 9 || n > 9 {
        return None;
    }

    let mut dp = [[0u32; 10]; 10];
    for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
        row[0] = (i as u32) * 26;
    }
    for (j, item) in dp[0].iter_mut().enumerate().take(n + 1) {
        *item = (j as u32) * 26;
    }
    for i in 1..=m {
        let sc = s_bytes[i - 1] as char;
        for j in 1..=n {
            let tc = t_bytes[j - 1] as char;
            let sub_cost = char_substitution_cost(sc, tc);
            let cost_sub = dp[i - 1][j - 1] + sub_cost;
            let cost_del = dp[i - 1][j] + 22;
            let cost_ins = dp[i][j - 1] + 22;
            dp[i][j] = cost_sub.min(cost_del).min(cost_ins);
        }
    }
    Some(dp[m][n])
}

/// 计算任意 Unicode 字符序列的加权编辑距离（通用堆上动态规划路径）
fn chars_weighted_edit_distance(s_chars: &[char], t_chars: &[char]) -> u32 {
    let m = s_chars.len();
    let n = t_chars.len();

    if m == 0 {
        return (n as u32) * 26;
    }
    if n == 0 {
        return (m as u32) * 26;
    }
    if m.abs_diff(n) > 2 {
        return 100;
    }

    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
        row[0] = (i as u32) * 26;
    }
    for (j, item) in dp[0].iter_mut().enumerate().take(n + 1) {
        *item = (j as u32) * 26;
    }

    for i in 1..=m {
        let sc = s_chars[i - 1];
        for j in 1..=n {
            let tc = t_chars[j - 1];
            let sub_cost = char_substitution_cost(sc, tc);
            let cost_sub = dp[i - 1][j - 1] + sub_cost;
            let cost_del = dp[i - 1][j] + 22;
            let cost_ins = dp[i][j - 1] + 22;

            dp[i][j] = cost_sub.min(cost_del).min(cost_ins);
        }
    }

    dp[m][n]
}

/// 获取输入拼音片段基于物理键盘加权的高斯纠错候选音节静态切片列表（零字符串堆分配）
pub(crate) fn get_spatial_typo_corrections_static(
    slice: &str,
    max_cost: u32,
) -> Vec<(&'static str, u32)> {
    if slice.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::with_capacity(8);
    let slice_len = slice.len();
    let first_char = slice.chars().next().unwrap_or('\0');

    for &std_syl in STANDARD_SYLLABLES {
        if std_syl.len().abs_diff(slice_len) > 1 {
            continue;
        }
        // 距离平方阈值 2.25 等价于距离 1.5，彻底免去热路径浮点开方
        if let Some(target_first) = std_syl.chars().next()
            && key_spatial_distance_sq(first_char, target_first) > 2.25
        {
            continue;
        }
        let cost = weighted_edit_distance(slice, std_syl);
        if cost > 0 && cost <= max_cost {
            candidates.push((std_syl, cost));
        }
    }

    candidates.sort_by_key(|item| item.1);
    candidates.truncate(4);
    candidates
}

/// 获取输入拼音片段基于物理键盘加权的高斯纠错候选音节列表
///
/// 限制最大返回代价 `max_cost`（默认 25），返回 `(合法标准音节, 编辑代价)` 列表，按代价升序排列
pub fn get_spatial_typo_corrections(slice: &str, max_cost: u32) -> Vec<(String, u32)> {
    get_spatial_typo_corrections_static(slice, max_cost)
        .into_iter()
        .map(|(s, cost)| (s.to_string(), cost))
        .collect()
}

/// 获取输入拼音片段的通用容错纠错音节列表
///
/// # 覆盖场景
/// 1. 韵母易混颠倒 (ie <=> ei, ui <=> iu)；
/// 2. 鼻音与辅音快速键入倒置 (neg => eng, agn => ang, ign => ing, ogn => ong)；
/// 3. 元音轻微丢失或错按 (uen => un)。
pub fn get_typo_corrections(slice: &str) -> Vec<String> {
    get_typo_corrections_with_rules(slice, &TypoCorrectionRules::default())
}

/// 在指定纠错规则下生成输入拼音片段的容错纠错音节列表
///
/// 每个细分规则只负责自己声明的变换；主开关关闭时直接返回空列表。
pub fn get_typo_corrections_with_rules(slice: &str, rules: &TypoCorrectionRules) -> Vec<String> {
    let mut results = Vec::new();
    for_each_typo_correction_with_rules(slice, rules, |cand| {
        if !results.iter().any(|existing: &String| existing == cand) {
            results.push(cand.to_string());
        }
    });
    results
}

/// 在栈上零堆分配构造规则纠错候选并通过回调逐个输出（供音节格构建热路径使用）
pub(crate) fn for_each_typo_correction_with_rules(
    slice: &str,
    rules: &TypoCorrectionRules,
    mut on_match: impl FnMut(&str),
) {
    if !rules.enable || slice.is_empty() || slice.len() > 12 || !slice.is_ascii() {
        return;
    }

    let mut stack = [0u8; 16];

    // 1. 韵母易混颠倒
    if slice.contains("ie") {
        emit_replaced(slice, "ie", "ei", &mut stack, &mut on_match);
    } else if slice.contains("ei") {
        emit_replaced(slice, "ei", "ie", &mut stack, &mut on_match);
    }
    if slice.contains("ui") {
        emit_replaced(slice, "ui", "iu", &mut stack, &mut on_match);
    } else if slice.contains("iu") {
        emit_replaced(slice, "iu", "ui", &mut stack, &mut on_match);
    }

    // 2. 鼻音与辅音快速键入倒置
    if rules.gn_ng {
        if let Some(prefix) = slice.strip_suffix("gn") {
            emit_suffixed(slice, prefix, "ng", &mut stack, &mut on_match);
        }
        if let Some(prefix) = slice.strip_suffix("neg") {
            emit_suffixed(slice, prefix, "eng", &mut stack, &mut on_match);
        }
    }
    if rules.mg_ng
        && let Some(prefix) = slice.strip_suffix("mg")
    {
        emit_suffixed(slice, prefix, "ng", &mut stack, &mut on_match);
    }

    // 3. 韵母缩写与 ü 系列输入习惯变换
    if rules.iou_iu && slice.contains("iou") {
        emit_replaced(slice, "iou", "iu", &mut stack, &mut on_match);
    }
    if rules.uei_ui && slice.contains("uei") {
        emit_replaced(slice, "uei", "ui", &mut stack, &mut on_match);
    }
    if rules.uen_un && slice.contains("uen") {
        emit_replaced(slice, "uen", "un", &mut stack, &mut on_match);
    }
    if rules.ue_ve && slice.contains("ue") {
        emit_replaced(slice, "ue", "ve", &mut stack, &mut on_match);
    }
    if rules.v_u {
        if slice.contains('v') {
            emit_replaced(slice, "v", "u", &mut stack, &mut on_match);
        }
        if slice.contains("ve") {
            emit_replaced(slice, "ve", "ue", &mut stack, &mut on_match);
        }
        if slice.contains("van") {
            emit_replaced(slice, "van", "uan", &mut stack, &mut on_match);
        }
    }
    if rules.on_ong
        && let Some(prefix) = slice.strip_suffix("on")
    {
        emit_suffixed(slice, prefix, "ong", &mut stack, &mut on_match);
    }
}

/// 在栈缓冲中执行首次子串替换并校验标准音节合法性
fn emit_replaced(
    slice: &str,
    from: &str,
    to: &str,
    stack: &mut [u8; 16],
    on_match: &mut impl FnMut(&str),
) {
    let Some(pos) = slice.find(from) else {
        return;
    };
    let tail = &slice[pos + from.len()..];
    let total = pos + to.len() + tail.len();
    if total > stack.len() {
        return;
    }
    stack[..pos].copy_from_slice(&slice.as_bytes()[..pos]);
    stack[pos..pos + to.len()].copy_from_slice(to.as_bytes());
    stack[pos + to.len()..total].copy_from_slice(tail.as_bytes());
    if let Ok(cand) = std::str::from_utf8(&stack[..total])
        && cand != slice
        && is_valid_syllable(cand)
    {
        on_match(cand);
    }
}

/// 在栈缓冲中拼接前缀与替换后缀并校验标准音节合法性
fn emit_suffixed(
    slice: &str,
    prefix: &str,
    suffix: &str,
    stack: &mut [u8; 16],
    on_match: &mut impl FnMut(&str),
) {
    let total = prefix.len() + suffix.len();
    if total > stack.len() {
        return;
    }
    stack[..prefix.len()].copy_from_slice(prefix.as_bytes());
    stack[prefix.len()..total].copy_from_slice(suffix.as_bytes());
    if let Ok(cand) = std::str::from_utf8(&stack[..total])
        && cand != slice
        && is_valid_syllable(cand)
    {
        on_match(cand);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corrections(slice: &str, rules: TypoCorrectionRules) -> Vec<String> {
        get_typo_corrections_with_rules(slice, &rules)
    }

    #[test]
    fn test_typo_rules_individual_switches() {
        let enabled = TypoCorrectionRules::default();
        assert!(corrections("liagn", enabled).iter().any(|s| s == "liang"));
        assert!(corrections("liamg", enabled).iter().any(|s| s == "liang"));
        assert!(corrections("niou", enabled).iter().any(|s| s == "niu"));
        assert!(corrections("duei", enabled).iter().any(|s| s == "dui"));
        assert!(corrections("luen", enabled).iter().any(|s| s == "lun"));
        assert!(corrections("lue", enabled).iter().any(|s| s == "lve"));
        assert!(corrections("lv", enabled).iter().any(|s| s == "lu"));
        assert!(corrections("don", enabled).iter().any(|s| s == "dong"));
    }

    #[test]
    fn test_typo_rules_can_be_disabled_individually() {
        let gn_off = TypoCorrectionRules {
            gn_ng: false,
            ..Default::default()
        };
        assert!(!corrections("liagn", gn_off).iter().any(|s| s == "liang"));

        let mg_off = TypoCorrectionRules {
            mg_ng: false,
            ..Default::default()
        };
        assert!(!corrections("liamg", mg_off).iter().any(|s| s == "liang"));

        let iou_off = TypoCorrectionRules {
            iou_iu: false,
            ..Default::default()
        };
        assert!(!corrections("niou", iou_off).iter().any(|s| s == "niu"));

        let uei_off = TypoCorrectionRules {
            uei_ui: false,
            ..Default::default()
        };
        assert!(!corrections("duei", uei_off).iter().any(|s| s == "dui"));

        let uen_off = TypoCorrectionRules {
            uen_un: false,
            ..Default::default()
        };
        assert!(!corrections("luen", uen_off).iter().any(|s| s == "lun"));

        let ue_off = TypoCorrectionRules {
            ue_ve: false,
            ..Default::default()
        };
        assert!(!corrections("lue", ue_off).iter().any(|s| s == "lve"));

        let v_off = TypoCorrectionRules {
            v_u: false,
            ..Default::default()
        };
        assert!(!corrections("lv", v_off).iter().any(|s| s == "lu"));

        let on_off = TypoCorrectionRules {
            on_ong: false,
            ..Default::default()
        };
        assert!(!corrections("don", on_off).iter().any(|s| s == "dong"));
    }

    #[test]
    fn test_typo_rules_master_switch_disables_all() {
        let rules = TypoCorrectionRules {
            enable: false,
            ..Default::default()
        };
        assert!(corrections("liagn", rules).is_empty());
        assert!(corrections("lue", rules).is_empty());
    }
}
