use crate::types::AlgoCandidate;
use chrono::{Datelike, Local, Timelike};

/// 动态时间与日期评估器 (DateTime Evaluator)
///
/// # 设计原理
/// - **实现初衷**：1:1 对标微信键盘的动态时间、日期与扩展卡片格式能力。
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
        let now = Local::now();

        match clean.as_str() {
            "riqi" | "rq" => {
                let subs = Self::format_date_formats(now);
                Some(
                    AlgoCandidate::new("日期", pinyin, 999_999)
                        .with_comment("按Tab/下键展开")
                        .with_sub_candidates(subs),
                )
            }
            "shijian" | "sj" => {
                let subs = Self::format_time_formats(now);
                Some(
                    AlgoCandidate::new("时间", pinyin, 999_999)
                        .with_comment("按Tab/下键展开")
                        .with_sub_candidates(subs),
                )
            }
            "riqishijian" | "rqsj" => {
                let subs = Self::format_datetime_formats(now);
                Some(
                    AlgoCandidate::new("日期时间", pinyin, 999_999)
                        .with_comment("按Tab/下键展开")
                        .with_sub_candidates(subs),
                )
            }
            "xingqi" | "xq" | "zhou" => {
                let subs = Self::format_weekday_formats(now);
                Some(
                    AlgoCandidate::new("星期", pinyin, 999_999)
                        .with_comment("按Tab/下键展开")
                        .with_sub_candidates(subs),
                )
            }
            _ => None,
        }
    }

    /// 格式化全量日期格式列表（对标微信键盘图3）
    pub fn format_date_formats(now: chrono::DateTime<Local>) -> Vec<String> {
        let year = now.year();
        let month = now.month();
        let day = now.day();

        vec![
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
            // 7. 农历/干支纪年: 丙午[马]年八月廿九
            Self::to_lunar_chinese_str(year, month, day),
        ]
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

    /// 格式化完整日期时间戳列表（对标微信键盘图4）
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
        let digits = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
        let mut year_str = String::new();
        for ch in year.to_string().chars() {
            if let Some(d) = ch.to_digit(10) {
                year_str.push(digits[d as usize]);
            }
        }

        let month_str = match month {
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
        };

        let day_str = match day {
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
        };

        format!("{}年{}{}", year_str, month_str, day_str)
    }

    /// 计算干支年及生肖（如：丙午[马]年）与农历月日（基于 1900-2100 标准农历数据表精确推算）
    fn to_lunar_chinese_str(year: i32, month: u32, day: u32) -> String {
        const TIAN_GAN: [&str; 10] = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"];
        const DI_ZHI: [&str; 12] = [
            "子", "丑", "寅", "卯", "辰", "巳", "午", "未", "申", "酉", "戌", "亥",
        ];
        const SHENG_XIAO: [&str; 12] = [
            "鼠", "牛", "虎", "兔", "龙", "蛇", "马", "羊", "猴", "鸡", "狗", "猪",
        ];

        let (lunar_year, is_leap, lunar_month, lunar_day) =
            Self::solar_to_lunar(year, month, day).unwrap_or((year, false, month, day));

        let gan_idx = (((lunar_year - 4) % 10 + 10) % 10) as usize;
        let zhi_idx = (((lunar_year - 4) % 12 + 12) % 12) as usize;

        let ganzhi_year = format!(
            "{}{}[{}]年",
            TIAN_GAN[gan_idx], DI_ZHI[zhi_idx], SHENG_XIAO[zhi_idx]
        );

        let (lunar_month_str, lunar_day_str) = Self::format_lunar_month_day(lunar_month, lunar_day);
        let leap_prefix = if is_leap { "闰" } else { "" };
        format!(
            "{}{}{}{}",
            ganzhi_year, leap_prefix, lunar_month_str, lunar_day_str
        )
    }

    /// 格式化农历月份与农历日名称
    fn format_lunar_month_day(lunar_month: u32, lunar_day: u32) -> (&'static str, &'static str) {
        const MONTH_NAMES: [&str; 12] = [
            "正月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "冬月",
            "腊月",
        ];
        const DAY_NAMES: [&str; 30] = [
            "初一", "初二", "初三", "初四", "初五", "初六", "初七", "初八", "初九", "初十", "十一",
            "十二", "十三", "十四", "十五", "十六", "十七", "十八", "十九", "二十", "廿一", "廿二",
            "廿三", "廿四", "廿五", "廿六", "廿七", "廿八", "廿九", "三十",
        ];

        let m_idx = lunar_month.saturating_sub(1) as usize % 12;
        let d_idx = lunar_day.saturating_sub(1) as usize % 30;
        (MONTH_NAMES[m_idx], DAY_NAMES[d_idx])
    }

    /// 基于 1900–2100 年紧凑农历位编码表将公历年月日换算为 `(农历年, 是否闰月, 农历月, 农历日)`
    fn solar_to_lunar(year: i32, month: u32, day: u32) -> Option<(i32, bool, u32, u32)> {
        // 1900–2100 年农历朔望月与闰月编码表（基准日：1900年1月31日 = 农历庚子年正月初一）
        const LUNAR_INFO: [u32; 201] = [
            0x04bd8, 0x04ae0, 0x0a570, 0x054d5, 0x0d260, 0x0d950, 0x16554, 0x056a0, 0x09ad0,
            0x055d2, 0x04ae0, 0x0a5b6, 0x0a4d0, 0x0d250, 0x1d255, 0x0b540, 0x0d6a0, 0x0ada2,
            0x095b0, 0x14977, 0x04970, 0x0a4b0, 0x0b4b5, 0x06a50, 0x06d40, 0x1ab54, 0x02b60,
            0x09570, 0x052f2, 0x04970, 0x06566, 0x0d4a0, 0x0ea50, 0x06e95, 0x05ad0, 0x02b60,
            0x186e3, 0x092e0, 0x1c8d7, 0x0c950, 0x0d4a0, 0x1d8a6, 0x0b550, 0x056a0, 0x1a5b4,
            0x025d0, 0x092d0, 0x0d2b2, 0x0a950, 0x0b557, 0x06ca0, 0x0b550, 0x15355, 0x04da0,
            0x0a5b0, 0x14573, 0x052b0, 0x0a9a8, 0x0e950, 0x06aa0, 0x0aea6, 0x0ab50, 0x04b60,
            0x0aae4, 0x0a570, 0x05260, 0x0f263, 0x0d950, 0x05b57, 0x056a0, 0x096d0, 0x04dd5,
            0x04ad0, 0x0a4d0, 0x0d4d4, 0x0d250, 0x0d558, 0x0b540, 0x0b6a0, 0x195a6, 0x095b0,
            0x049b0, 0x0a974, 0x0a4b0, 0x0b27a, 0x06a50, 0x06d40, 0x0af46, 0x0ab60, 0x09570,
            0x04af5, 0x04970, 0x064b0, 0x074a3, 0x0ea50, 0x06b58, 0x05ac0, 0x0ab60, 0x096d5,
            0x092e0, 0x0c960, 0x0d954, 0x0d4a0, 0x0da50, 0x07552, 0x056a0, 0x0abb7, 0x025d0,
            0x092d0, 0x0cab5, 0x0a950, 0x0b4a0, 0x0baa4, 0x0ad50, 0x055d9, 0x04ba0, 0x0a5b0,
            0x15176, 0x052b0, 0x0a930, 0x07954, 0x06aa0, 0x0ad50, 0x05b52, 0x04b60, 0x0a6e6,
            0x0a4e0, 0x0d260, 0x0ea65, 0x0d530, 0x05aa0, 0x076a3, 0x096d0, 0x04afb, 0x04ad0,
            0x0a4d0, 0x1d0b6, 0x0d250, 0x0d520, 0x0dd45, 0x0b5a0, 0x056d0, 0x055b2, 0x049b0,
            0x0a577, 0x0a4b0, 0x0aa50, 0x1b255, 0x06d20, 0x0ada0, 0x14b63, 0x09370, 0x049f8,
            0x04970, 0x064b0, 0x168a6, 0x0ea50, 0x06aa0, 0x1a6c4, 0x0aae0, 0x092e0, 0x0d2e3,
            0x0c960, 0x0d557, 0x0d4a0, 0x0da50, 0x05d55, 0x056a0, 0x0a6d0, 0x055d4, 0x052d0,
            0x0a9b8, 0x0a950, 0x0b4a0, 0x0b6a6, 0x0ad50, 0x055a0, 0x0aba4, 0x0a5b0, 0x052b0,
            0x0b273, 0x06930, 0x07337, 0x06aa0, 0x0ad50, 0x14b55, 0x04b60, 0x0a570, 0x054e4,
            0x0d160, 0x0e968, 0x0d520, 0x0daa0, 0x16aa6, 0x056d0, 0x04ae0, 0x0a9d4, 0x0a2d0,
            0x0d150, 0x0f252, 0x0d520,
        ];

        let base_date = chrono::NaiveDate::from_ymd_opt(1900, 1, 31)?;
        let target_date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        let mut offset = target_date.signed_duration_since(base_date).num_days();
        if offset < 0 {
            return None;
        }

        let lunar_year_days = |info: u32| -> i64 {
            let mut sum = 348i64;
            let mut mask = 0x8000u32;
            while mask > 0x8 {
                if (info & mask) != 0 {
                    sum += 1;
                }
                mask >>= 1;
            }
            let leap_month = info & 0xf;
            if leap_month != 0 {
                sum += if (info & 0x10000) != 0 { 30 } else { 29 };
            }
            sum
        };

        let mut l_year = 1900i32;
        for &info in &LUNAR_INFO {
            let days = lunar_year_days(info);
            if offset < days {
                break;
            }
            offset -= days;
            l_year += 1;
        }
        let y_idx = (l_year - 1900) as usize;
        let info = *LUNAR_INFO.get(y_idx)?;
        let leap_month = info & 0xf;
        let mut is_leap = false;

        let mut l_month = 1u32;
        while l_month <= 12 {
            let days_in_month = if is_leap {
                if (info & 0x10000) != 0 {
                    30
                } else {
                    29
                }
            } else if (info & (0x10000 >> l_month)) != 0 {
                30
            } else {
                29
            };

            if offset < days_in_month {
                break;
            }
            offset -= days_in_month;

            if !is_leap && leap_month > 0 && l_month == leap_month {
                is_leap = true;
            } else {
                if is_leap {
                    is_leap = false;
                }
                l_month += 1;
            }
        }

        let l_day = (offset as u32).saturating_add(1);
        Some((l_year, is_leap, l_month, l_day))
    }
}
pub use crate::evaluator_financial::FinancialEvaluator;
