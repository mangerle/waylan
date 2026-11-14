//! 标准 QWERTY 键盘物理空间几何模型与加权纠错算法
//!
//! # 设计原理
//! - **实现初衷**：利用物理按键的几何欧式距离加权计算拼写纠错代价，使紧邻按键误触获得更低惩罚。
//! - **核心优势**：通过预计算二维高斯替换代价矩阵与邻键表，在运行期实现纯整数 O(1) 查表。

use super::standard::{is_valid_syllable, STANDARD_SYLLABLES};
use waylan_core::config::TypoCorrectionRules;

/// 获取标准 QWERTY 键盘字母的物理二维平面坐标 (x, y)
///
/// 坐标基于标准键盘物理中心距归一化：
/// - 行 0: Q~P 行 (y = 0.0)
/// - 行 1: A~L 行 (y = 1.0, x 轴右移约 0.25 错位)
/// - 行 2: Z~M 行 (y = 2.0, x 轴右移约 0.75 错位)
pub fn get_qwerty_coord(c: char) -> Option<(f32, f32)> {
    match c {
        'q' => Some((0.0, 0.0)),
        'w' => Some((1.0, 0.0)),
        'e' => Some((2.0, 0.0)),
        'r' => Some((3.0, 0.0)),
        't' => Some((4.0, 0.0)),
        'y' => Some((5.0, 0.0)),
        'u' => Some((6.0, 0.0)),
        'i' => Some((7.0, 0.0)),
        'o' => Some((8.0, 0.0)),
        'p' => Some((9.0, 0.0)),

        'a' => Some((0.25, 1.0)),
        's' => Some((1.25, 1.0)),
        'd' => Some((2.25, 1.0)),
        'f' => Some((3.25, 1.0)),
        'g' => Some((4.25, 1.0)),
        'h' => Some((5.25, 1.0)),
        'j' => Some((6.25, 1.0)),
        'k' => Some((7.25, 1.0)),
        'l' => Some((8.25, 1.0)),

        'z' => Some((0.75, 2.0)),
        'x' => Some((1.75, 2.0)),
        'c' => Some((2.75, 2.0)),
        'v' => Some((3.75, 2.0)),
        'b' => Some((4.75, 2.0)),
        'n' => Some((5.75, 2.0)),
        'm' => Some((6.75, 2.0)),
        _ => None,
    }
}

/// 获取标准 QWERTY 键盘字母在 0..26 范围内的索引
#[inline]
pub fn ascii_letter_idx(c: char) -> Option<usize> {
    if c.is_ascii_lowercase() {
        Some((c as u8 - b'a') as usize)
    } else if c.is_ascii_uppercase() {
        Some((c as u8 - b'A') as usize)
    } else {
        None
    }
}

/// 标准 QWERTY 物理键盘 26x26 二维空间替换高斯惩罚代价矩阵
///
/// 矩阵由物理几何坐标差经高斯衰减预计算，消除运行期浮点与指数运算
#[rustfmt::skip]
pub static QWERTY_SPATIAL_COST_MATRIX: [[u8; 26]; 26] = [
    [0, 45, 44, 40, 40, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 24, 45, 24, 45, 45, 45, 29, 38, 45, 26], // a
    [45, 0, 40, 44, 45, 38, 26, 26, 45, 38, 44, 45, 40, 24, 45, 45, 45, 44, 45, 41, 43, 24, 45, 45, 40, 45], // b
    [44, 40, 0, 26, 41, 26, 38, 44, 45, 45, 45, 45, 45, 45, 45, 45, 45, 40, 38, 43, 45, 24, 44, 24, 45, 40], // c
    [40, 44, 26, 0, 24, 24, 40, 45, 45, 45, 45, 45, 45, 45, 45, 45, 43, 29, 24, 40, 45, 38, 35, 26, 45, 38], // d
    [40, 45, 41, 24, 0, 35, 43, 45, 45, 45, 45, 45, 45, 45, 45, 45, 40, 24, 29, 40, 45, 44, 24, 40, 45, 43], // e
    [45, 38, 26, 24, 35, 0, 24, 40, 45, 45, 45, 45, 45, 44, 45, 45, 45, 24, 40, 29, 45, 26, 43, 38, 40, 44], // f
    [45, 26, 38, 40, 43, 24, 0, 24, 45, 40, 45, 45, 44, 38, 45, 45, 45, 35, 45, 24, 40, 26, 45, 44, 29, 45], // g
    [45, 26, 44, 45, 45, 40, 24, 0, 40, 24, 40, 45, 38, 26, 45, 45, 45, 43, 45, 35, 29, 38, 45, 45, 24, 45], // h
    [45, 45, 45, 45, 45, 45, 45, 40, 0, 29, 24, 35, 40, 43, 24, 40, 45, 45, 45, 45, 24, 45, 45, 45, 40, 45], // i
    [45, 38, 45, 45, 45, 45, 40, 24, 29, 0, 24, 40, 26, 26, 40, 45, 45, 45, 45, 43, 24, 44, 45, 45, 35, 45], // j
    [45, 44, 45, 45, 45, 45, 45, 40, 24, 24, 0, 24, 26, 38, 29, 40, 45, 45, 45, 45, 35, 45, 45, 45, 43, 45], // k
    [45, 45, 45, 45, 45, 45, 45, 45, 35, 40, 24, 0, 38, 44, 24, 29, 45, 45, 45, 45, 43, 45, 45, 45, 45, 45], // l
    [45, 40, 45, 45, 45, 45, 44, 38, 40, 26, 26, 38, 0, 24, 43, 45, 45, 45, 45, 45, 41, 45, 45, 45, 44, 45], // m
    [45, 24, 45, 45, 45, 44, 38, 26, 43, 26, 38, 44, 24, 0, 45, 45, 45, 45, 45, 44, 40, 40, 45, 45, 41, 45], // n
    [45, 45, 45, 45, 45, 45, 45, 45, 24, 40, 29, 24, 43, 45, 0, 24, 45, 45, 45, 45, 40, 45, 45, 45, 45, 45], // o
    [45, 45, 45, 45, 45, 45, 45, 45, 40, 45, 40, 29, 45, 45, 24, 0, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45], // p
    [24, 45, 45, 43, 40, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 0, 45, 35, 45, 45, 45, 24, 44, 45, 41], // q
    [45, 44, 40, 29, 24, 24, 35, 43, 45, 45, 45, 45, 45, 45, 45, 45, 45, 0, 40, 24, 45, 41, 40, 43, 40, 45], // r
    [24, 45, 38, 24, 29, 40, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 35, 40, 0, 45, 45, 44, 24, 26, 45, 26], // s
    [45, 41, 43, 40, 40, 29, 24, 35, 45, 43, 45, 45, 45, 44, 45, 45, 45, 24, 45, 0, 40, 40, 45, 45, 24, 45], // t
    [45, 43, 45, 45, 45, 45, 40, 29, 24, 24, 35, 43, 41, 40, 40, 45, 45, 45, 45, 40, 0, 45, 45, 45, 24, 45], // u
    [45, 24, 24, 38, 44, 26, 26, 38, 45, 44, 45, 45, 45, 40, 45, 45, 45, 41, 44, 40, 45, 0, 45, 40, 43, 45], // v
    [29, 45, 44, 35, 24, 43, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 24, 40, 24, 45, 45, 45, 0, 41, 45, 40], // w
    [38, 45, 24, 26, 40, 38, 44, 45, 45, 45, 45, 45, 45, 45, 45, 45, 44, 43, 26, 45, 45, 40, 41, 0, 45, 24], // x
    [45, 40, 45, 45, 45, 40, 29, 24, 40, 35, 43, 45, 44, 41, 45, 45, 45, 40, 45, 24, 24, 43, 45, 45, 0, 45], // y
    [26, 45, 40, 38, 43, 44, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 41, 45, 26, 45, 45, 45, 40, 24, 45, 0], // z
];

/// 物理键盘紧邻与次邻按键预计算查找表 (邻键字符, 高斯几何代价)
#[rustfmt::skip]
pub static QWERTY_SPATIAL_NEIGHBORS: [&[(char, u8)]; 26] = [
    &[('q', 24), ('s', 24), ('z', 26), ('w', 29)],                         // a
    &[('n', 24), ('v', 24), ('g', 26), ('h', 26)],                         // b
    &[('v', 24), ('x', 24), ('d', 26), ('f', 26)],                         // c
    &[('e', 24), ('f', 24), ('s', 24), ('c', 26), ('x', 26), ('r', 29)], // d
    &[('d', 24), ('r', 24), ('w', 24), ('s', 29)],                         // e
    &[('d', 24), ('g', 24), ('r', 24), ('c', 26), ('v', 26), ('t', 29)], // f
    &[('f', 24), ('h', 24), ('t', 24), ('b', 26), ('v', 26), ('y', 29)], // g
    &[('g', 24), ('j', 24), ('y', 24), ('b', 26), ('n', 26), ('u', 29)], // h
    &[('k', 24), ('o', 24), ('u', 24), ('j', 29)],                         // i
    &[('h', 24), ('k', 24), ('u', 24), ('m', 26), ('n', 26), ('i', 29)], // j
    &[('i', 24), ('j', 24), ('l', 24), ('m', 26), ('o', 29)],             // k
    &[('k', 24), ('o', 24), ('p', 29)],                                     // l
    &[('n', 24), ('j', 26), ('k', 26)],                                     // m
    &[('b', 24), ('m', 24), ('h', 26), ('j', 26)],                         // n
    &[('i', 24), ('l', 24), ('p', 24), ('k', 29)],                         // o
    &[('o', 24), ('l', 29)],                                                 // p
    &[('a', 24), ('w', 24)],                                                 // q
    &[('e', 24), ('f', 24), ('t', 24), ('d', 29)],                         // r
    &[('a', 24), ('d', 24), ('w', 24), ('x', 26), ('z', 26), ('e', 29)], // s
    &[('g', 24), ('r', 24), ('y', 24), ('f', 29)],                         // t
    &[('i', 24), ('j', 24), ('y', 24), ('h', 29)],                         // u
    &[('b', 24), ('c', 24), ('f', 26), ('g', 26)],                         // v
    &[('e', 24), ('q', 24), ('s', 24), ('a', 29)],                         // w
    &[('c', 24), ('z', 24), ('d', 26), ('s', 26)],                         // x
    &[('h', 24), ('t', 24), ('u', 24), ('g', 29)],                         // y
    &[('x', 24), ('a', 26), ('s', 26)],                                     // z
];

/// 获取按键在物理键盘上的紧邻误触按键列表及对应惩罚代价
#[inline]
pub fn get_qwerty_neighbors(c: char) -> &'static [(char, u8)] {
    if let Some(idx) = ascii_letter_idx(c) {
        QWERTY_SPATIAL_NEIGHBORS[idx]
    } else {
        &[]
    }
}

/// 计算两按键在物理键盘上的几何欧氏距离
pub fn key_spatial_distance(c1: char, c2: char) -> f32 {
    if c1 == c2 {
        return 0.0;
    }
    match (get_qwerty_coord(c1), get_qwerty_coord(c2)) {
        (Some((x1, y1)), Some((x2, y2))) => {
            let dx = x1 - x2;
            let dy = y1 - y2;
            (dx * dx + dy * dy).sqrt()
        }
        _ => 5.0,
    }
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
    if source.is_ascii() && target.is_ascii() {
        if let Some(distance) = ascii_weighted_edit_distance(source.as_bytes(), target.as_bytes()) {
            return distance;
        }
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

/// 获取输入拼音片段基于物理键盘加权的高斯纠错候选音节列表
///
/// 限制最大返回代价 `max_cost`（默认 25），返回 `(合法标准音节, 编辑代价)` 列表，按代价升序排列
pub fn get_spatial_typo_corrections(slice: &str, max_cost: u32) -> Vec<(String, u32)> {
    if slice.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let slice_len = slice.len();
    let first_char = slice.chars().next().unwrap_or('\0');

    for &std_syl in STANDARD_SYLLABLES {
        // 长度差超过 1 快速剪枝
        if std_syl.len().abs_diff(slice_len) > 1 {
            continue;
        }
        // 声母首字母物理距离相差过大快速剪枝（仅探索紧邻误触按键）
        if let Some(target_first) = std_syl.chars().next() {
            if key_spatial_distance(first_char, target_first) > 1.5 {
                continue;
            }
        }
        let cost = weighted_edit_distance(slice, std_syl);
        if cost > 0 && cost <= max_cost {
            candidates.push((std_syl.to_string(), cost));
        }
    }

    candidates.sort_by_key(|item| item.1);
    candidates.truncate(4);
    candidates
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
    if !rules.enable || slice.is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();

    // 1. 韵母易混颠倒属于通用拼写容错，受主开关控制
    collect_rhyme_swap_typos(slice, &mut results);

    // 2. 鼻音与辅音快速键入倒置
    collect_nasal_typos(slice, rules, &mut results);

    // 3. 韵母缩写与 ü 系列输入习惯变换
    collect_abbrev_typos(slice, rules, &mut results);

    results
}

/// 收集韵母易混颠倒的容错音节（ie/ei 与 ui/iu 双向互换）
fn collect_rhyme_swap_typos(slice: &str, results: &mut Vec<String>) {
    if slice.contains("ie") {
        push_valid_typo(results, slice, slice.replace("ie", "ei"));
    } else if slice.contains("ei") {
        push_valid_typo(results, slice, slice.replace("ei", "ie"));
    }
    if slice.contains("ui") {
        push_valid_typo(results, slice, slice.replace("ui", "iu"));
    } else if slice.contains("iu") {
        push_valid_typo(results, slice, slice.replace("iu", "ui"));
    }
}

/// 收集鼻音与辅音快速键入倒置的容错音节（gn、neg、mg 尾部误触归一）
fn collect_nasal_typos(slice: &str, rules: &TypoCorrectionRules, results: &mut Vec<String>) {
    if rules.gn_ng {
        if let Some(prefix) = slice.strip_suffix("gn") {
            push_valid_typo(results, slice, format!("{}ng", prefix));
        }
        if let Some(prefix) = slice.strip_suffix("neg") {
            push_valid_typo(results, slice, format!("{}eng", prefix));
        }
    }
    if rules.mg_ng {
        if let Some(prefix) = slice.strip_suffix("mg") {
            push_valid_typo(results, slice, format!("{}ng", prefix));
        }
    }
}

/// 收集韵母缩写与 ü 系列输入习惯变换的容错音节
fn collect_abbrev_typos(slice: &str, rules: &TypoCorrectionRules, results: &mut Vec<String>) {
    if rules.iou_iu && slice.contains("iou") {
        push_valid_typo(results, slice, slice.replace("iou", "iu"));
    }
    if rules.uei_ui && slice.contains("uei") {
        push_valid_typo(results, slice, slice.replace("uei", "ui"));
    }
    if rules.uen_un && slice.contains("uen") {
        push_valid_typo(results, slice, slice.replace("uen", "un"));
    }
    if rules.ue_ve && slice.contains("ue") {
        push_valid_typo(results, slice, slice.replace("ue", "ve"));
    }
    if rules.v_u {
        if slice.contains('v') {
            push_valid_typo(results, slice, slice.replace('v', "u"));
        }
        if slice.contains("ve") {
            push_valid_typo(results, slice, slice.replace("ve", "ue"));
        }
        if slice.contains("van") {
            push_valid_typo(results, slice, slice.replace("van", "uan"));
        }
    }
    if rules.on_ong {
        if let Some(prefix) = slice.strip_suffix("on") {
            push_valid_typo(results, slice, format!("{}ong", prefix));
        }
    }
}

/// 将合法且不重复的纠错音节追加到结果列表
fn push_valid_typo(results: &mut Vec<String>, slice: &str, alternative: String) {
    if alternative != slice && is_valid_syllable(&alternative) && !results.contains(&alternative) {
        results.push(alternative);
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
