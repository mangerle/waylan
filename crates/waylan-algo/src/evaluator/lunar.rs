//! 农历天文历法推算与干支纪年模块
//!
//! # 设计原理
//! - **实现初衷**：为日期与时间快捷指令提供高精度的中国传统农历、生肖与天干地支纪年表达。
//! - **核心优势**：基于 1900–2100 年紧凑 17 位位编码表，零外部庞大天文依赖，精确推算闰月与大小月。
//! - **代价与局限**：历法覆盖范围为 1900 年至 2100 年，超出范围安全返回 `None`。

const TIAN_GAN: [&str; 10] = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"];
const DI_ZHI: [&str; 12] = [
    "子", "丑", "寅", "卯", "辰", "巳", "午", "未", "申", "酉", "戌", "亥",
];
const SHENG_XIAO: [&str; 12] = [
    "鼠", "牛", "虎", "兔", "龙", "蛇", "马", "羊", "猴", "鸡", "狗", "猪",
];

/// 1900–2100 年农历朔望月与闰月编码表（基准日：1900年1月31日 = 农历庚子年正月初一）
const LUNAR_INFO: [u32; 201] = [
    0x04bd8, 0x04ae0, 0x0a570, 0x054d5, 0x0d260, 0x0d950, 0x16554, 0x056a0, 0x09ad0, 0x055d2,
    0x04ae0, 0x0a5b6, 0x0a4d0, 0x0d250, 0x1d255, 0x0b540, 0x0d6a0, 0x0ada2, 0x095b0, 0x14977,
    0x04970, 0x0a4b0, 0x0b4b5, 0x06a50, 0x06d40, 0x1ab54, 0x02b60, 0x09570, 0x052f2, 0x04970,
    0x06566, 0x0d4a0, 0x0ea50, 0x06e95, 0x05ad0, 0x02b60, 0x186e3, 0x092e0, 0x1c8d7, 0x0c950,
    0x0d4a0, 0x1d8a6, 0x0b550, 0x056a0, 0x1a5b4, 0x025d0, 0x092d0, 0x0d2b2, 0x0a950, 0x0b557,
    0x06ca0, 0x0b550, 0x15355, 0x04da0, 0x0a5b0, 0x14573, 0x052b0, 0x0a9a8, 0x0e950, 0x06aa0,
    0x0aea6, 0x0ab50, 0x04b60, 0x0aae4, 0x0a570, 0x05260, 0x0f263, 0x0d950, 0x05b57, 0x056a0,
    0x096d0, 0x04dd5, 0x04ad0, 0x0a4d0, 0x0d4d4, 0x0d250, 0x0d558, 0x0b540, 0x0b6a0, 0x195a6,
    0x095b0, 0x049b0, 0x0a974, 0x0a4b0, 0x0b27a, 0x06a50, 0x06d40, 0x0af46, 0x0ab60, 0x09570,
    0x04af5, 0x04970, 0x064b0, 0x074a3, 0x0ea50, 0x06b58, 0x05ac0, 0x0ab60, 0x096d5, 0x092e0,
    0x0c960, 0x0d954, 0x0d4a0, 0x0da50, 0x07552, 0x056a0, 0x0abb7, 0x025d0, 0x092d0, 0x0cab5,
    0x0a950, 0x0b4a0, 0x0baa4, 0x0ad50, 0x055d9, 0x04ba0, 0x0a5b0, 0x15176, 0x052b0, 0x0a930,
    0x07954, 0x06aa0, 0x0ad50, 0x05b52, 0x04b60, 0x0a6e6, 0x0a4e0, 0x0d260, 0x0ea65, 0x0d530,
    0x05aa0, 0x076a3, 0x096d0, 0x04afb, 0x04ad0, 0x0a4d0, 0x1d0b6, 0x0d250, 0x0d520, 0x0dd45,
    0x0b5a0, 0x056d0, 0x055b2, 0x049b0, 0x0a577, 0x0a4b0, 0x0aa50, 0x1b255, 0x06d20, 0x0ada0,
    0x14b63, 0x09370, 0x049f8, 0x04970, 0x064b0, 0x168a6, 0x0ea50, 0x06aa0, 0x1a6c4, 0x0aae0,
    0x092e0, 0x0d2e3, 0x0c960, 0x0d557, 0x0d4a0, 0x0da50, 0x05d55, 0x056a0, 0x0a6d0, 0x055d4,
    0x052d0, 0x0a9b8, 0x0a950, 0x0b4a0, 0x0b6a6, 0x0ad50, 0x055a0, 0x0aba4, 0x0a5b0, 0x052b0,
    0x0b273, 0x06930, 0x07337, 0x06aa0, 0x0ad50, 0x14b55, 0x04b60, 0x0a570, 0x054e4, 0x0d160,
    0x0e968, 0x0d520, 0x0daa0, 0x16aa6, 0x056d0, 0x04ae0, 0x0a9d4, 0x0a2d0, 0x0d150, 0x0f252,
    0x0d520,
];

/// 计算干支年及生肖（如：丙午[马]年）与农历月日（基于 1900-2100 标准农历数据表精确推算）
pub fn to_lunar_chinese_str(year: i32, month: u32, day: u32) -> Option<String> {
    let (lunar_year, is_leap, lunar_month, lunar_day) = solar_to_lunar(year, month, day)?;

    let gan_idx = (((lunar_year - 4) % 10 + 10) % 10) as usize;
    let zhi_idx = (((lunar_year - 4) % 12 + 12) % 12) as usize;

    let ganzhi_year = format!(
        "{}{}[{}]年",
        TIAN_GAN[gan_idx], DI_ZHI[zhi_idx], SHENG_XIAO[zhi_idx]
    );

    let (lunar_month_str, lunar_day_str) = format_lunar_month_day(lunar_month, lunar_day);
    let leap_prefix = if is_leap { "闰" } else { "" };
    Some(format!(
        "{}{}{}{}",
        ganzhi_year, leap_prefix, lunar_month_str, lunar_day_str
    ))
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
    let base_date = chrono::NaiveDate::from_ymd_opt(1900, 1, 31)?;
    let target_date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let mut offset = target_date.signed_duration_since(base_date).num_days();
    if offset < 0 {
        return None;
    }

    let l_year = locate_lunar_year(&mut offset);
    let y_idx = (l_year - 1900) as usize;
    let info = *LUNAR_INFO.get(y_idx)?;
    let (l_month, is_leap) = locate_lunar_month(info, &mut offset);

    let l_day = (offset as u32).saturating_add(1);
    Some((l_year, is_leap, l_month, l_day))
}

/// 计算农历编码表条目所代表年份的总天数（含闰月天数）
fn lunar_year_days(info: u32) -> i64 {
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
}

/// 依据自基准日起的天数偏移定位所属农历年，并扣除已完整走过的整年天数
fn locate_lunar_year(offset: &mut i64) -> i32 {
    let mut l_year = 1900i32;
    for &info in &LUNAR_INFO {
        let days = lunar_year_days(info);
        if *offset < days {
            break;
        }
        *offset -= days;
        l_year += 1;
    }
    l_year
}

/// 依据农历年编码与剩余天数偏移定位农历月份，返回月序与是否闰月
fn locate_lunar_month(info: u32, offset: &mut i64) -> (u32, bool) {
    let leap_month = info & 0xf;
    let mut is_leap = false;
    let mut l_month = 1u32;

    while l_month <= 12 {
        let days_in_month = if is_leap {
            if (info & 0x10000) != 0 { 30 } else { 29 }
        } else if (info & (0x10000 >> l_month)) != 0 {
            30
        } else {
            29
        };

        if *offset < days_in_month {
            break;
        }
        *offset -= days_in_month;

        if !is_leap && leap_month > 0 && l_month == leap_month {
            is_leap = true;
        } else {
            if is_leap {
                is_leap = false;
            }
            l_month += 1;
        }
    }

    (l_month, is_leap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lunar_chinese_str_bounds() {
        let valid_lunar = to_lunar_chinese_str(2026, 10, 9);
        assert!(valid_lunar.is_some());
        assert!(valid_lunar.unwrap().contains("丙午[马]年"));

        assert_eq!(to_lunar_chinese_str(1899, 1, 1), None);
        assert_eq!(to_lunar_chinese_str(2102, 1, 1), None);
    }
}
