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
    "lo", "long", "lou", "lu", "luan", "lue", "lun", "luo", "lv", "ma", "mai", "man", "mang",
    "mao", "me", "mei", "men", "meng", "mi", "mian", "miao", "mie", "min", "ming", "miu", "mo",
    "mou", "mu", "na", "nai", "nan", "nang", "nao", "ne", "nei", "nen", "neng", "ni", "nian",
    "niang", "niao", "nie", "nin", "ning", "niu", "nong", "nou", "nu", "nuan", "nue", "nuo", "nv",
    "o", "ou", "pa", "pai", "pan", "pang", "pao", "pei", "pen", "peng", "pi", "pian", "piao",
    "pie", "pin", "ping", "po", "pou", "pu", "qi", "qia", "qian", "qiang", "qiao", "qie", "qin",
    "qing", "qiong", "qiu", "qu", "quan", "que", "qun", "ran", "rang", "rao", "re", "ren", "reng",
    "ri", "rong", "rou", "ru", "rua", "ruan", "rui", "run", "ruo", "sa", "sai", "san", "sang",
    "sao", "se", "sen", "seng", "sha", "shai", "shan", "shang", "shao", "she", "shei", "shen",
    "sheng", "shi", "shou", "shu", "shua", "shuai", "shuan", "shuang", "shui", "shun", "shuo",
    "si", "song", "sou", "su", "suan", "sui", "sun", "suo", "ta", "tai", "tan", "tang", "tao",
    "te", "teng", "ti", "tian", "tiao", "tie", "ting", "tong", "tou", "tu", "tuan", "tui", "tun",
    "tuo", "wa", "wai", "wan", "wang", "wei", "wen", "weng", "wo", "wu", "xi", "xia", "xian",
    "xiang", "xiao", "xie", "xin", "xing", "xiong", "xiu", "xu", "xuan", "xue", "xun", "ya", "yan",
    "yang", "yao", "ye", "yi", "yin", "ying", "yo", "yong", "you", "yu", "yuan", "yue", "yun",
    "za", "zai", "zan", "zang", "zao", "ze", "zei", "zen", "zeng", "zha", "zhai", "zhan", "zhang",
    "zhao", "zhe", "zhei", "zhen", "zheng", "zhi", "zhong", "zhou", "zhu", "zhua", "zhuai",
    "zhuan", "zhuang", "zhui", "zhun", "zhuo", "zi", "zong", "zou", "zu", "zuan", "zui", "zun",
    "zuo",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_syllables() {
        assert!(is_valid_syllable("ni"));
        assert!(is_valid_syllable("hao"));
        assert!(is_valid_syllable("xian"));
        assert!(is_valid_syllable("zhong"));
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
}
