//! 拼音音节定义、QWERTY 物理键盘空间几何模型与加权纠错算法
//!
//! # 架构设计
//! - `standard`: 汉语拼音 410 个合法标准音节常量表及二分判定、前缀匹配与无分配分词硬剪枝算法；
//! - `spatial`: 基于标准 QWERTY 键盘物理坐标高斯衰减计算的按键空间代价矩阵与加权编辑距离纠错。

pub mod spatial;
pub mod standard;

pub use spatial::*;
pub use standard::*;

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
