use waylan_core::is_cjk_character;

/// 盘古之白（中西文智能空格）与中文量词过滤模块
pub struct PanguSpacing;

impl PanguSpacing {
    /// 检查两个相邻文本之间是否需要插入盘古之白（中西文智能空格）
    pub fn should_insert_pangu_space(prev_context: &str, curr_commit: &str) -> bool {
        let prev_text = prev_context.trim();
        let curr_text = curr_commit.trim();

        if prev_text.is_empty() || curr_text.is_empty() {
            return false;
        }

        let prev_ch = match prev_text.chars().last() {
            Some(c) => c,
            None => return false,
        };

        let curr_ch = match curr_text.chars().next() {
            Some(c) => c,
            None => return false,
        };

        // 若前驱结尾或当前开头已有空格，则不重复插入
        if prev_context.ends_with(' ') || curr_commit.starts_with(' ') {
            return false;
        }

        // 场景 1：中文汉字接英文单词或数字（如："测试" + "Rust" => "测试 Rust"；"第" + "10" => "第 10"）
        if is_cjk_character(prev_ch) && (curr_ch.is_ascii_alphabetic() || curr_ch.is_ascii_digit())
        {
            return true;
        }

        // 场景 2：英文单词接中文汉字（如："Rust" + "代码" => "Rust 代码"）
        if prev_ch.is_ascii_alphabetic() && is_cjk_character(curr_ch) {
            return true;
        }

        // 场景 3：数字接中文汉字（如："100" + "苹果" => "100 苹果"）
        // 豁免情况：若当前词为中文量词或单位（如："100" + "米" => "100米"），则不插入空格
        if prev_ch.is_ascii_digit() && is_cjk_character(curr_ch) {
            if Self::is_chinese_classifier(curr_text) {
                return false;
            }
            return true;
        }

        false
    }

    /// 判断是否属于常见中文量词或单位（用于数字与中文紧邻时的空格豁免）
    pub fn is_chinese_classifier(word: &str) -> bool {
        const CLASSIFIERS: &[&str] = &[
            // 常见单字量词
            "个",
            "只",
            "支",
            "枝",
            "张",
            "条",
            "头",
            "匹",
            "尾",
            "块",
            "片",
            "粒",
            "颗",
            "本",
            "册",
            "部",
            "首",
            "篇",
            "幅",
            "座",
            "栋",
            "扇",
            "套",
            "件",
            "双",
            "对",
            "副",
            "打",
            "群",
            "把",
            "口",
            "根",
            "排",
            "行",
            "段",
            "节",
            "项",
            "类",
            "种",
            "束",
            "串",
            "包",
            "盒",
            "箱",
            "袋",
            "瓶",
            "罐",
            "杯",
            "碗",
            "盘",
            "锅",
            "桶",
            "盆",
            "页",
            "枚",
            "尊",
            "通",
            "场",
            "顿",
            "番",
            "次",
            "回",
            "趟",
            "遍",
            "遭",
            "阵",
            "门",
            "桩",
            "任",
            "届",
            // 度量衡单位
            "米",
            "分米",
            "厘米",
            "毫米",
            "微米",
            "纳米",
            "千米",
            "公里",
            "里",
            "尺",
            "寸",
            "丈",
            "克",
            "千克",
            "公斤",
            "斤",
            "两",
            "钱",
            "吨",
            "磅",
            "盎司",
            "升",
            "毫升",
            "立方米",
            "平方米",
            "平方",
            "公顷",
            "亩",
            // 时间与日期
            "秒",
            "分",
            "分钟",
            "刻",
            "时",
            "小时",
            "天",
            "日",
            "周",
            "月",
            "年",
            "岁",
            "代",
            "季",
            "度",
            // 货币与比率
            "元",
            "角",
            "块",
            "毛",
            "镑",
            "刀",
            "折",
            "倍",
            "成",
            "点",
            "级",
            "百分点",
            "万",
            "亿",
            "兆",
        ];

        CLASSIFIERS.contains(&word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pangu_spacing_classifier_and_cjk() {
        // 数字接单字量词/单位豁免空格
        assert!(!PanguSpacing::should_insert_pangu_space("100", "个"));
        assert!(!PanguSpacing::should_insert_pangu_space("100", "米"));
        assert!(!PanguSpacing::should_insert_pangu_space("5", "分钟"));

        // 数字接以量词开头的普通名词不得豁免空格
        assert!(PanguSpacing::should_insert_pangu_space("100", "个人"));
        assert!(PanguSpacing::should_insert_pangu_space("100", "点心"));
        assert!(PanguSpacing::should_insert_pangu_space("10", "门口"));

        // 中西文之间应插入空格
        assert!(PanguSpacing::should_insert_pangu_space("测试", "Rust"));
        assert!(PanguSpacing::should_insert_pangu_space("Rust", "代码"));

        // 汉字零“〇”应正确识别为 CJK 字符
        assert!(PanguSpacing::should_insert_pangu_space("二〇二六", "Rust"));
    }
}
