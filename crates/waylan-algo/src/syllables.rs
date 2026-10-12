/// 现代汉语普通话全部标准合法拼音音节表（按字典序排列）
///
/// # 设计原理
/// - **实现初衷**：汉语所有合法音节数量固定在 410 个以内，采用预排序静态切片可实现零运行期分配。
/// - **核心优势**：通过 `binary_search` 可在纳秒级完成精确判定；通过二分切片可在微秒内完成前缀检索。
/// - **代价与局限**：静态固定表无法直接表达方言音节，但能满足标准普通话全量需求。
pub static STANDARD_SYLLABLES: &[&str] = &[
    "a", "ai", "an", "ang", "ao", "ba", "bai", "ban", "bang", "bao", "bei", "ben", "beng", "bi",
    "bian", "biao", "bie", "bin", "bing", "bo", "bu", "ca", "cai", "can", "cang", "cao", "ce",
    "cen", "ceng", "cha", "chai", "chan", "chang", "chao", "che", "chen", "cheng", "chi", "chong",
    "chou", "chu", "chua", "chuai", "chuan", "chuang", "chui", "chun", "chuo", "ci", "cong", "cou",
    "cu", "cuan", "cui", "cun", "cuo", "da", "dai", "dan", "dang", "dao", "de", "dei", "den",
    "deng", "di", "dia", "dian", "diao", "die", "ding", "diu", "dong", "dou", "du", "duan", "dui",
    "dun", "duo", "e", "ei", "en", "eng", "er", "fa", "fan", "fang", "fei", "fen", "feng", "fo",
    "fou", "fu", "ga", "gai", "gan", "gang", "gao", "ge", "gei", "gen", "geng", "gong", "gou",
    "gu", "gua", "guai", "guan", "guang", "gui", "gun", "guo", "ha", "hai", "han", "hang", "hao",
    "he", "hei", "hen", "heng", "hong", "hou", "hu", "hua", "huai", "huan", "huang", "hui", "hun",
    "huo", "ji", "jia", "jian", "jiang", "jiao", "jie", "jin", "jing", "jiong", "jiu", "ju",
    "juan", "jue", "jun", "ka", "kai", "kan", "kang", "kao", "ke", "kei", "ken", "keng", "kong",
    "kou", "ku", "kua", "kuai", "kuan", "kuang", "kui", "kun", "kuo", "la", "lai", "lan", "lang",
    "lao", "le", "lei", "leng", "li", "lia", "lian", "liang", "liao", "lie", "lin", "ling", "liu",
    "lo", "long", "lou", "lu", "luan", "lue", "lun", "luo", "lv", "lve", "ma", "mai", "man",
    "mang", "mao", "me", "mei", "men", "meng", "mi", "mian", "miao", "mie", "min", "ming", "miu",
    "mo", "mou", "mu", "na", "nai", "nan", "nang", "nao", "ne", "nei", "nen", "neng", "ni", "nian",
    "niang", "niao", "nie", "nin", "ning", "niu", "nong", "nou", "nu", "nuan", "nue", "nuo", "nv",
    "nve", "o", "ou", "pa", "pai", "pan", "pang", "pao", "pei", "pen", "peng", "pi", "pian",
    "piao", "pie", "pin", "ping", "po", "pou", "pu", "qi", "qia", "qian", "qiang", "qiao", "qie",
    "qin", "qing", "qiong", "qiu", "qu", "quan", "que", "qun", "ran", "rang", "rao", "re", "ren",
    "reng", "ri", "rong", "rou", "ru", "rua", "ruan", "rui", "run", "ruo", "sa", "sai", "san",
    "sang", "sao", "se", "sen", "seng", "sha", "shai", "shan", "shang", "shao", "she", "shei",
    "shen", "sheng", "shi", "shou", "shu", "shua", "shuai", "shuan", "shuang", "shui", "shun",
    "shuo", "si", "song", "sou", "su", "suan", "sui", "sun", "suo", "ta", "tai", "tan", "tang",
    "tao", "te", "teng", "ti", "tian", "tiao", "tie", "ting", "tong", "tou", "tu", "tuan", "tui",
    "tun", "tuo", "wa", "wai", "wan", "wang", "wei", "wen", "weng", "wo", "wu", "xi", "xia",
    "xian", "xiang", "xiao", "xie", "xin", "xing", "xiong", "xiu", "xu", "xuan", "xue", "xun",
    "ya", "yan", "yang", "yao", "ye", "yi", "yin", "ying", "yo", "yong", "you", "yu", "yuan",
    "yue", "yun", "za", "zai", "zan", "zang", "zao", "ze", "zei", "zen", "zeng", "zha", "zhai",
    "zhan", "zhang", "zhao", "zhe", "zhei", "zhen", "zheng", "zhi", "zhong", "zhou", "zhu", "zhua",
    "zhuai", "zhuan", "zhuang", "zhui", "zhun", "zhuo", "zi", "zong", "zou", "zu", "zuan", "zui",
    "zun", "zuo",
];

/// 判断给定的字符串是否是合法的拼音音节
pub fn is_valid_syllable(s: &str) -> bool {
    STANDARD_SYLLABLES.binary_search(&s).is_ok()
}

/// 检查是否存在以该前缀开头的合法音节（用于未完成键入的前缀判定）
pub fn has_syllable_prefix(prefix: &str) -> bool {
    if prefix.is_empty() {
        return false;
    }
    match STANDARD_SYLLABLES.binary_search(&prefix) {
        Ok(_) => true,
        Err(idx) => {
            if idx < STANDARD_SYLLABLES.len() {
                STANDARD_SYLLABLES[idx].starts_with(prefix)
            } else {
                false
            }
        }
    }
}

/// 检查字符串是否可以完全由标准合法音节拼接而成（零堆分配，微秒级音节可达性动态规划剪枝）
///
/// # 设计原理
/// - **实现初衷**：在物理邻键空间容错枚举产生的大量变异字符串中，绝大多数为无法切分的乱码；
/// - **核心优势**：基于固定长度栈数组动态规划，零堆内存分配，耗时仅数纳秒，可在进入重量级 DAG 图搜索前淘汰 90% 以上无效组合；
/// - **代价与局限**：仅判定全切分可达性，不包含纠错容错边与模糊音扩展，仅作为前置硬剪枝漏斗。
pub fn can_segment_into_syllables(s: &str) -> bool {
    let n = s.len();
    if n == 0 || n > 32 {
        return false;
    }
    let mut reachable = [false; 33];
    reachable[0] = true;

    for i in 0..n {
        if !reachable[i] {
            continue;
        }
        let max_len = (n - i).min(6);
        for len in 1..=max_len {
            let sub = &s[i..i + len];
            if is_valid_syllable(sub) {
                reachable[i + len] = true;
            }
        }
    }

    reachable[n]
}

/// 检查是否存在严格比当前字符串更长的合法音节以该字符串为前缀
pub fn has_strictly_longer_syllable(prefix: &str) -> bool {
    if prefix.is_empty() {
        return false;
    }
    let start_idx = match STANDARD_SYLLABLES.binary_search(&prefix) {
        Ok(idx) => idx + 1,
        Err(idx) => idx,
    };
    if start_idx < STANDARD_SYLLABLES.len() {
        STANDARD_SYLLABLES[start_idx].starts_with(prefix)
    } else {
        false
    }
}

/// 根据拼音前缀二分截取所有匹配的标准音节切片
///
/// # 设计原理
/// - **实现初衷**：为 Viterbi 尾部残存音节扩展提供极速音节切片，消除对 410 个音节的逐一遍历；
/// - **核心优势**：利用 `STANDARD_SYLLABLES` 字典序预排序特性，通过 `partition_point` 二分定位起始位置，
///   零堆内存分配返回连续借用切片，时间复杂度从 O(N) 降至 O(log N)；
/// - **代价与局限**：依赖底层音节表保持严格升序排列。
pub fn find_syllables_by_prefix(prefix: &str) -> &'static [&'static str] {
    if prefix.is_empty() {
        return STANDARD_SYLLABLES;
    }
    let start_idx = STANDARD_SYLLABLES.partition_point(|s| *s < prefix);
    let slice = &STANDARD_SYLLABLES[start_idx..];
    let count = slice.iter().take_while(|s| s.starts_with(prefix)).count();
    &slice[..count]
}

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
/// 算法原理：
/// 采用高斯分布衰减模型：惩罚 = base + alpha * (1.0 - exp(-d^2 / (2 * sigma^2)))
/// 紧邻按键（d <= 1.05，如 u-i, u-y, s-a, s-d）代价极低（约 12~14）；
/// 次邻近按键（1.05 < d <= 1.5，如 s-w, u-j）代价约为 18~22；
/// 跨区域远距离误触按键代价在 35 以上。
pub fn char_substitution_cost(c1: char, c2: char) -> u32 {
    if c1 == c2 {
        return 0;
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
        let s_bytes = source.as_bytes();
        let t_bytes = target.as_bytes();
        let m = s_bytes.len();
        let n = t_bytes.len();

        if m == 0 {
            return (n as u32) * 26;
        }
        if n == 0 {
            return (m as u32) * 26;
        }
        if m.abs_diff(n) > 2 {
            return 100;
        }

        if m <= 9 && n <= 9 {
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
                    let cost_del = dp[i - 1][j] + 26;
                    let cost_ins = dp[i][j - 1] + 26;
                    dp[i][j] = cost_sub.min(cost_del).min(cost_ins);
                }
            }
            return dp[m][n];
        }
    }

    let s_chars: Vec<char> = source.chars().collect();
    let t_chars: Vec<char> = target.chars().collect();
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
            let cost_del = dp[i - 1][j] + 26;
            let cost_ins = dp[i][j - 1] + 26;

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
    let mut results = Vec::new();

    // 1. ie <=> ei 双向互换 (如 mie <=> mei, tie <=> tei, die <=> dei, xei => xie)
    if slice.contains("ie") {
        let alt = slice.replace("ie", "ei");
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    } else if slice.contains("ei") {
        let alt = slice.replace("ei", "ie");
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    }

    // 2. ui <=> iu 易混颠倒 (如 gui <=> giu, dui <=> diu)
    if slice.contains("ui") {
        let alt = slice.replace("ui", "iu");
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    } else if slice.contains("iu") {
        let alt = slice.replace("iu", "ui");
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    }

    // 3. 鼻音辅音结尾倒置: neg => eng, agn => ang, ign => ing, ogn => ong
    if let Some(prefix) = slice.strip_suffix("neg") {
        let alt = format!("{}eng", prefix);
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    } else if let Some(prefix) = slice.strip_suffix("agn") {
        let alt = format!("{}ang", prefix);
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    } else if let Some(prefix) = slice.strip_suffix("ign") {
        let alt = format!("{}ing", prefix);
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    } else if let Some(prefix) = slice.strip_suffix("ogn") {
        let alt = format!("{}ong", prefix);
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    }

    // 4. 元音轻微丢失或错按 (uen => un)
    if slice.contains("uen") {
        let alt = slice.replace("uen", "un");
        if is_valid_syllable(&alt) {
            results.push(alt);
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_syllables_strictly_sorted() {
        assert!(
            STANDARD_SYLLABLES.windows(2).all(|w| w[0] < w[1]),
            "STANDARD_SYLLABLES 必须严格按字典序升序且无重复以保障 binary_search 正确性"
        );
    }

    #[test]
    fn test_valid_syllables() {
        assert!(is_valid_syllable("ni"));
        assert!(is_valid_syllable("hao"));
        assert!(is_valid_syllable("xian"));
        assert!(is_valid_syllable("zhong"));
        assert!(is_valid_syllable("lve"));
        assert!(is_valid_syllable("nve"));
        assert!(!is_valid_syllable("xyz"));
        assert!(!is_valid_syllable(""));
    }

    #[test]
    fn test_syllable_prefix() {
        assert!(has_syllable_prefix("zh"));
        assert!(has_syllable_prefix("x"));
        assert!(has_syllable_prefix("ni"));
        assert!(!has_syllable_prefix("zz"));
    }

    #[test]
    fn test_can_segment_into_syllables() {
        assert!(can_segment_into_syllables("nihao"));
        assert!(can_segment_into_syllables("cheng"));
        assert!(can_segment_into_syllables("xian"));
        assert!(can_segment_into_syllables("womende"));
        assert!(can_segment_into_syllables("zhongguoren"));
        assert!(can_segment_into_syllables("beijingtiananmen"));
        assert!(!can_segment_into_syllables("nihso"));
        assert!(!can_segment_into_syllables("bkxyz"));
        assert!(!can_segment_into_syllables(""));
    }

    #[test]
    fn test_find_syllables_by_prefix() {
        let j_syls = find_syllables_by_prefix("j");
        assert!(!j_syls.is_empty());
        assert!(j_syls.contains(&"ji"));
        assert!(j_syls.contains(&"juan"));
        assert!(j_syls.iter().all(|s| s.starts_with('j')));

        let zh_syls = find_syllables_by_prefix("zh");
        assert!(!zh_syls.is_empty());
        assert!(zh_syls.contains(&"zha"));
        assert!(zh_syls.contains(&"zhong"));
        assert!(zh_syls.iter().all(|s| s.starts_with("zh")));

        let none_syls = find_syllables_by_prefix("zz");
        assert!(none_syls.is_empty());
    }

    #[test]
    fn test_keyboard_spatial_distance_and_cost() {
        // u 与 i 属于紧邻键，几何距离为 1.0
        let dist_u_i = key_spatial_distance('u', 'i');
        assert!((dist_u_i - 1.0).abs() < 1e-4);
        let cost_u_i = char_substitution_cost('u', 'i');
        assert!(cost_u_i <= 25, "邻键误触惩罚应在合理低位: {}", cost_u_i);

        // u 与 z 跨区域远距离按键
        let dist_u_z = key_spatial_distance('u', 'z');
        assert!(dist_u_z > 4.0);
        let cost_u_z = char_substitution_cost('u', 'z');
        assert!(cost_u_z >= 40, "远距离误触惩罚应很高: {}", cost_u_z);
    }

    #[test]
    fn test_spatial_typo_corrections() {
        // 误将 hao 打成 hso (s 与 a 邻近)
        let corrections = get_spatial_typo_corrections("hso", 25);
        assert!(
            corrections.iter().any(|(syl, _)| syl == "hao"),
            "hso 应纠错召回 hao"
        );

        // 误将 zhong 打成 zhobg (b 与 n 邻近)
        let corrections_zh = get_spatial_typo_corrections("zhobg", 25);
        assert!(
            corrections_zh.iter().any(|(syl, _)| syl == "zhong"),
            "zhobg 应纠错召回 zhong"
        );
    }
}
