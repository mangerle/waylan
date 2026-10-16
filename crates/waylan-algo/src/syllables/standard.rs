//! 现代汉语普通话全部标准合法拼音音节表与前缀判定算法
//!
//! # 设计原理
//! - **实现初衷**：汉语所有合法音节数量固定在 410 个以内，采用预排序静态切片可实现零运行期分配。
//! - **核心优势**：通过 `binary_search` 可在纳秒级完成精确判定；通过二分切片可在微秒内完成前缀检索。
//! - **代价与局限**：静态固定表无法直接表达方言音节，但能满足标准普通话全量需求。

/// 现代汉语普通话全部标准合法拼音音节表（按字典序排列）
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
