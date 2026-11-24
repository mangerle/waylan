use crate::types::AlgoCandidate;
use chrono::{Datelike, Local, Timelike};

/// 动态时间与日期评估器 (DateTime Evaluator)
///
/// # 设计原理
/// - **实现初衷**：提供丰富的动态时间、日期与扩展卡片格式能力。
/// - **核心优势**：零复杂外部库依赖，内置标准中文大写转换、干支纪年与农历算法；
///   流式生成 7 种以上细分格式，提供二级展开卡片支持。
#[derive(Debug, Default)]
pub struct DateTimeEvaluator;

impl DateTimeEvaluator {
    /// 检查输入拼音是否命中日期/时间扩展指令
    ///
    /// 若命中，生成携带二级子格式列表（sub_candidates）的动态功能候选条目
    pub fn try_evaluate(pinyin: &str) -> Option<AlgoCandidate> {
        let clean = pinyin.replace('\'', "").to_lowercase();
        match clean.as_str() {
            "riqi" | "rq" => {
                let now = Local::now();
                let subs = Self::format_date_formats(now);
                let first_text = subs.first().cloned().unwrap_or_else(|| "日期".to_string());
                Some(AlgoCandidate::new(first_text, pinyin, 999_999).with_sub_candidates(subs))
            }
            "shijian" | "sj" => {
                let now = Local::now();
                let subs = Self::format_time_formats(now);
                let first_text = subs.first().cloned().unwrap_or_else(|| "时间".to_string());
                Some(AlgoCandidate::new(first_text, pinyin, 999_999).with_sub_candidates(subs))
            }
            "riqishijian" | "rqsj" => {
                let now = Local::now();
                let subs = Self::format_datetime_formats(now);
                let first_text = subs
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "日期时间".to_string());
                Some(AlgoCandidate::new(first_text, pinyin, 999_999).with_sub_candidates(subs))
            }
            "xingqi" | "xq" | "zhou" => {
                let now = Local::now();
                let subs = Self::format_weekday_formats(now);
                let first_text = subs.first().cloned().unwrap_or_else(|| "星期".to_string());
                Some(AlgoCandidate::new(first_text, pinyin, 999_999).with_sub_candidates(subs))
            }
            _ => None,
        }
    }

    /// 格式化全量日期格式列表
    pub fn format_date_formats(now: chrono::DateTime<Local>) -> Vec<String> {
        let year = now.year();
        let month = now.month();
        let day = now.day();

        let mut list = vec![
            // 1. 中文常用格式: 2026年10月9日
            format!("{}年{}月{}日", year, month, day),
            // 2. 紧凑纯数字: 20261009
            format!("{:04}{:02}{:02}", year, month, day),
            // 3. 国际连字符: 2026-10-09
            format!("{:04}-{:02}-{:02}", year, month, day),
            // 4. 斜杠格式: 2026/10/09
            format!("{:04}/{:02}/{:02}", year, month, day),
            // 5. 点号格式: 2026.10.09
            format!("{:04}.{:02}.{:02}", year, month, day),
            // 6. 中文全大写: 二〇二六年十月九日
            Self::to_chinese_date(year, month, day),
        ];

        // 7. 农历/干支纪年: 丙午[马]年八月廿九（若超出 1900-2100 农历推算范围安全略过）
        if let Some(lunar_str) = Self::to_lunar_chinese_str(year, month, day) {
            list.push(lunar_str);
        }

        list
    }

    /// 格式化全量时间格式列表
    pub fn format_time_formats(now: chrono::DateTime<Local>) -> Vec<String> {
        let hour = now.hour();
        let min = now.minute();
        let sec = now.second();
        let year = now.year();
        let month = now.month();
        let day = now.day();

        vec![
            // 1. 标准时间: 16:03:26
            format!("{:02}:{:02}:{:02}", hour, min, sec),
            // 2. 中文时间: 16时03分26秒
            format!("{:02}时{:02}分{:02}秒", hour, min, sec),
            // 3. 紧凑纯数字: 160326
            format!("{:02}{:02}{:02}", hour, min, sec),
            // 4. 简略时间: 16:03
            format!("{:02}:{:02}", hour, min),
            // 5. 完整时间戳: 2026-10-09 16:03:26
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                year, month, day, hour, min, sec
            ),
            // 6. 中文全称时间戳: 2026年10月9日 16:03:26
            format!(
                "{}年{}月{}日 {:02}:{:02}:{:02}",
                year, month, day, hour, min, sec
            ),
        ]
    }

    /// 格式化完整日期时间戳列表
    pub fn format_datetime_formats(now: chrono::DateTime<Local>) -> Vec<String> {
        let year = now.year();
        let month = now.month();
        let day = now.day();
        let hour = now.hour();
        let min = now.minute();
        let sec = now.second();

        vec![
            // 1. 中文紧凑时间戳: 2026年10月9日16:03:26
            format!(
                "{}年{}月{}日{:02}:{:02}:{:02}",
                year, month, day, hour, min, sec
            ),
            // 2. 国际标准: 2026-10-09 16:03:26
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                year, month, day, hour, min, sec
            ),
            // 3. 紧凑数字: 20261009160326
            format!(
                "{:04}{:02}{:02}{:02}{:02}{:02}",
                year, month, day, hour, min, sec
            ),
            // 4. 纯时间: 16:03:26
            format!("{:02}:{:02}:{:02}", hour, min, sec),
        ]
    }

    /// 格式化星期列表
    pub fn format_weekday_formats(now: chrono::DateTime<Local>) -> Vec<String> {
        let weekday_cn = match now.weekday() {
            chrono::Weekday::Mon => ("星期一", "周一", "礼拜一", "Monday"),
            chrono::Weekday::Tue => ("星期二", "周二", "礼拜二", "Tuesday"),
            chrono::Weekday::Wed => ("星期三", "周三", "礼拜三", "Wednesday"),
            chrono::Weekday::Thu => ("星期四", "周四", "礼拜四", "Thursday"),
            chrono::Weekday::Fri => ("星期五", "周五", "礼拜五", "Friday"),
            chrono::Weekday::Sat => ("星期六", "周六", "礼拜六", "Saturday"),
            chrono::Weekday::Sun => ("星期日", "周日", "礼拜天", "Sunday"),
        };

        vec![
            weekday_cn.0.to_string(),
            weekday_cn.1.to_string(),
            weekday_cn.2.to_string(),
            weekday_cn.3.to_string(),
        ]
    }

    /// 将数字公历日期转换为中文全大写（如：二〇二六年十月九日）
    fn to_chinese_date(year: i32, month: u32, day: u32) -> String {
        let year_str = Self::chinese_year_digits(year);
        let month_str = Self::chinese_month_name(month);
        let day_str = Self::chinese_day_name(day);
        format!("{}年{}{}", year_str, month_str, day_str)
    }

    /// 将公历年份逐位转换为中文大写数字（忽略非数字字符）
    fn chinese_year_digits(year: i32) -> String {
        const DIGITS: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
        let mut year_str = String::new();
        for ch in year.to_string().chars() {
            if let Some(d) = ch.to_digit(10) {
                year_str.push(DIGITS[d as usize]);
            }
        }
        year_str
    }

    /// 将公历月份映射为中文大写月份名称（越界值回退为一月）
    fn chinese_month_name(month: u32) -> &'static str {
        match month {
            1 => "一月",
            2 => "二月",
            3 => "三月",
            4 => "四月",
            5 => "五月",
            6 => "六月",
            7 => "七月",
            8 => "八月",
            9 => "九月",
            10 => "十月",
            11 => "十一月",
            12 => "十二月",
            _ => "一月",
        }
    }

    /// 将公历日映射为中文大写日名称（越界值回退为一日）
    fn chinese_day_name(day: u32) -> &'static str {
        match day {
            1 => "一日",
            2 => "二日",
            3 => "三日",
            4 => "四日",
            5 => "五日",
            6 => "六日",
            7 => "七日",
            8 => "八日",
            9 => "九日",
            10 => "十日",
            11 => "十一日",
            12 => "十二日",
            13 => "十三日",
            14 => "十四日",
            15 => "十五日",
            16 => "十六日",
            17 => "十七日",
            18 => "十八日",
            19 => "十九日",
            20 => "二十日",
            21 => "二十一日",
            22 => "二十二日",
            23 => "二十三日",
            24 => "二十四日",
            25 => "二十五日",
            26 => "二十六日",
            27 => "二十七日",
            28 => "二十八日",
            29 => "二十九日",
            30 => "三十日",
            31 => "三十一日",
            _ => "一日",
        }
    }

    /// 计算干支年及生肖（如：丙午[马]年）与农历月日（基于 1900-2100 标准农历数据表精确推算）
    pub fn to_lunar_chinese_str(year: i32, month: u32, day: u32) -> Option<String> {
        lunar::to_lunar_chinese_str(year, month, day)
    }
}

pub mod financial;
pub mod lunar;
pub mod math;

pub use financial::FinancialEvaluator;
pub use math::{MathEvaluator, format_math_val_str};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lunar_chinese_str_bounds() {
        // 在 1900-2100 范围内能正确计算农历
        let valid_lunar = DateTimeEvaluator::to_lunar_chinese_str(2026, 10, 9);
        assert!(valid_lunar.is_some());
        assert!(valid_lunar.unwrap().contains("丙午[马]年"));

        // 超出 1900-2100 范围时安全返回 None，不使用公历冒充农历
        assert_eq!(DateTimeEvaluator::to_lunar_chinese_str(1899, 1, 1), None);
        assert_eq!(DateTimeEvaluator::to_lunar_chinese_str(2102, 1, 1), None);
    }
}
