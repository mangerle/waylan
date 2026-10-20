use crate::types::AlgoCandidate;

/// 财务大写金额与规范数字评估器 (Financial Evaluator)
///
/// # 设计原理
/// - **实现初衷**：纯 Rust 高性能实现，无外部复杂依赖，提供便捷高效的 V 模式大写金额扩展。
/// - **核心优势**：支持人民币大写（元角分整）、规范中文大写数字与财务千分位格式；纯数值与字符串运算，零内存额外开销。
#[derive(Debug, Default)]
pub struct FinancialEvaluator;

impl FinancialEvaluator {
    /// 尝试将输入纯数字或金额转换为财务大写候选词列表
    pub fn try_evaluate(input: &str) -> Option<Vec<AlgoCandidate>> {
        let clean = input.trim();
        if clean.is_empty() {
            return None;
        }

        let (int_val, frac_val) = Self::parse_amount(clean)?;

        let rmb_capital = Self::to_rmb_capital(int_val, frac_val);
        let chinese_digits = Self::to_chinese_digits_capital(clean);
        let accounting_num = Self::to_accounting_format(int_val, frac_val);

        let mut candidates = Vec::with_capacity(3);
        candidates.push(AlgoCandidate::new(rmb_capital, input, 100_000).with_comment("人民币大写"));
        if let Some(ch_digits) = chinese_digits {
            candidates
                .push(AlgoCandidate::new(ch_digits, input, 95_000).with_comment("中文大写数字"));
        }
        candidates
            .push(AlgoCandidate::new(accounting_num, input, 90_000).with_comment("财务千分位"));

        Some(candidates)
    }

    /// 解析金额输入为整数部分和小数部分（最多支持两位小数）
    fn parse_amount(s: &str) -> Option<(u64, u8)> {
        if s.is_empty() || s.len() > 15 {
            return None;
        }

        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() > 2 {
            return None;
        }

        let int_str = parts[0];
        if int_str.is_empty() && parts.len() == 1 {
            return None;
        }

        let int_val = if int_str.is_empty() {
            0
        } else {
            if !int_str.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            int_str.parse::<u64>().ok()?
        };

        let frac_val = if parts.len() == 2 {
            let frac_str = parts[1];
            if frac_str.is_empty()
                || frac_str.len() > 2
                || !frac_str.chars().all(|c| c.is_ascii_digit())
            {
                return None;
            }
            if frac_str.len() == 1 {
                frac_str.parse::<u8>().ok()? * 10
            } else {
                frac_str.parse::<u8>().ok()?
            }
        } else {
            0
        };

        Some((int_val, frac_val))
    }

    /// 转换为人民币标准大写（壹万贰仟叁佰肆拾伍元陆角柒分整）
    pub fn to_rmb_capital(int_val: u64, frac_val: u8) -> String {
        const DIGITS: &[&str] = &["零", "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];

        if int_val == 0 && frac_val == 0 {
            return "零元整".to_string();
        }

        let mut result = String::new();

        if int_val > 0 {
            result.push_str(&Self::convert_integer_part(int_val));
            result.push('元');
        }

        let jiao = frac_val / 10;
        let fen = frac_val % 10;

        if jiao == 0 && fen == 0 {
            result.push('整');
        } else {
            if jiao > 0 {
                result.push_str(DIGITS[jiao as usize]);
                result.push('角');
            } else if int_val > 0 && fen > 0 {
                result.push('零');
            }

            if fen > 0 {
                result.push_str(DIGITS[fen as usize]);
                result.push('分');
            } else if jiao > 0 {
                result.push('整');
            }
        }

        result
    }

    /// 将整数部分转换为中文大写（分四位一节处理：万、亿、万亿、亿亿）
    fn convert_integer_part(mut num: u64) -> String {
        if num == 0 {
            return "零".to_string();
        }

        let mut sections = Vec::with_capacity(5);
        while num > 0 {
            sections.push((num % 10000) as u16);
            num /= 10000;
        }

        let mut res = String::new();
        let mut has_high_section = false;
        let mut has_zero_section = false;

        // 统计“亿”大节（idx = 2 与 idx = 3）是否包含非零数值
        let yi_has_val =
            sections.get(2).copied().unwrap_or(0) > 0 || sections.get(3).copied().unwrap_or(0) > 0;

        for (idx, &sec) in sections.iter().enumerate().rev() {
            if sec > 0 {
                if has_zero_section || (has_high_section && sec < 1000) {
                    res.push('零');
                }
                let sec_str = Self::convert_section(sec);
                res.push_str(&sec_str);
                Self::push_section_unit(&mut res, idx);

                has_high_section = true;
                has_zero_section = false;
            } else {
                // 若当前为亿节且整个亿大节非空（万亿节有值），离开亿大节时需补上“亿”单位
                if idx == 2 && yi_has_val {
                    res.push('亿');
                }

                if has_high_section {
                    has_zero_section = true;
                }
            }
        }

        res
    }

    /// 追加四位小节所归属的大节单位（万、亿、万亿、亿亿）
    fn push_section_unit(res: &mut String, idx: usize) {
        match idx {
            4 => res.push_str("亿亿"),
            3 => res.push('万'),
            2 => res.push('亿'),
            1 => res.push('万'),
            _ => {}
        }
    }

    /// 转换千以内的四位小节
    fn convert_section(num: u16) -> String {
        const DIGITS: &[&str] = &["零", "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];
        const POS_UNITS: &[&str] = &["", "拾", "佰", "仟"];

        let mut res = String::new();
        let mut zero_flag = false;
        let temp = num;

        let digits = [
            (temp / 1000) % 10,
            (temp / 100) % 10,
            (temp / 10) % 10,
            temp % 10,
        ];

        let mut started = false;
        for (i, &d) in digits.iter().enumerate() {
            let unit_idx = 3 - i;
            if d > 0 {
                if zero_flag {
                    res.push('零');
                    zero_flag = false;
                }
                res.push_str(DIGITS[d as usize]);
                res.push_str(POS_UNITS[unit_idx]);
                started = true;
            } else if started {
                zero_flag = true;
            }
        }

        res
    }

    /// 规范中文大写数字（如 12345.67 -> 壹万贰仟叁佰肆拾伍点陆柒）
    pub fn to_chinese_digits_capital(raw: &str) -> Option<String> {
        const DIGITS: &[&str] = &["零", "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];
        let parts: Vec<&str> = raw.split('.').collect();
        let int_str = parts[0];
        let int_val = if int_str.is_empty() {
            0
        } else {
            int_str.parse::<u64>().ok()?
        };
        let mut res = Self::convert_integer_part(int_val);

        if parts.len() == 2 && !parts[1].is_empty() {
            res.push('点');
            for c in parts[1].chars() {
                if let Some(digit) = c.to_digit(10) {
                    res.push_str(DIGITS[digit as usize]);
                }
            }
        }

        Some(res)
    }

    /// 财务千分位格式化（如 12345.67 -> 12,345.67）
    pub fn to_accounting_format(int_val: u64, frac_val: u8) -> String {
        let s = int_val.to_string();
        let mut formatted_int = String::new();
        let len = s.len();
        for (i, ch) in s.chars().enumerate() {
            formatted_int.push(ch);
            if (len - 1 - i).is_multiple_of(3) && i != len - 1 {
                formatted_int.push(',');
            }
        }
        format!("{}.{:02}", formatted_int, frac_val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_financial_trillion_and_billion_capital() {
        // 万亿级且亿节非零：不得重复输出“亿”
        assert_eq!(
            FinancialEvaluator::to_rmb_capital(1_0001_0000_0000, 0),
            "壹万零壹亿元整"
        );
        // 万亿整
        assert_eq!(
            FinancialEvaluator::to_rmb_capital(1_0000_0000_0000, 0),
            "壹万亿元整"
        );
        // 万亿带万节
        assert_eq!(
            FinancialEvaluator::to_rmb_capital(1_0000_0001_0000, 0),
            "壹万亿零壹万元整"
        );
        // 亿整
        assert_eq!(
            FinancialEvaluator::to_rmb_capital(1_0000_0000, 0),
            "壹亿元整"
        );
        // 常规金额带角分
        assert_eq!(
            FinancialEvaluator::to_rmb_capital(12345, 67),
            "壹万贰仟叁佰肆拾伍元陆角柒分"
        );
        // 仅有角
        assert_eq!(FinancialEvaluator::to_rmb_capital(0, 50), "伍角整");
        // 仅有分
        assert_eq!(FinancialEvaluator::to_rmb_capital(0, 5), "伍分");
    }
}
