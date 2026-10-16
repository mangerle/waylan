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
    if n == 0 || n > 32 || !s.is_ascii() {
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

    // 3. 鼻音辅音结尾倒置与通用 gn 误输: *gn => *ng (如 liagn => liang, zhegn => zheng, shuagn => shuang), neg => eng
    if let Some(prefix) = slice.strip_suffix("gn") {
        let alt = format!("{}ng", prefix);
        if is_valid_syllable(&alt) && !results.contains(&alt) {
            results.push(alt);
        }
    }
    if let Some(prefix) = slice.strip_suffix("neg") {
        let alt = format!("{}eng", prefix);
        if is_valid_syllable(&alt) && !results.contains(&alt) {
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

    #[test]
    fn test_can_segment_into_syllables_non_ascii_safety() {
        assert!(!can_segment_into_syllables("你好"));
        assert!(!can_segment_into_syllables("ni'好"));
        assert!(!can_segment_into_syllables("★※→"));
        assert!(can_segment_into_syllables("zhongguo"));
        assert!(can_segment_into_syllables("xian"));
        assert!(!can_segment_into_syllables("zzzzzz"));
    }
}
