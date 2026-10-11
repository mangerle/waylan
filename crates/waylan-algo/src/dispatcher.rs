use crate::evaluator::{DateTimeEvaluator, FinancialEvaluator};
use crate::types::{AlgoCandidate, AlgoConfig};

/// 模式分流器
///
/// # 设计原理
/// - **实现初衷**：在输入管道最前端侦测前缀控制字符（如 `v`, `u`, `i`），将控制流分发至对应微处理器，
///   避免在拼音主状态机中散落复杂的非拼音分支判断。
/// - **核心优势**：纯函数式处理，零全局状态开销，各个特殊模式可独立单测与扩展。
/// - **代价与局限**：前缀字符被占用（如输入以 v 开头的字符无法直接作为全拼），但与业界通用输入法习惯一致。
#[derive(Debug, Default)]
pub struct ModeDispatcher;

impl ModeDispatcher {
    /// 尝试按特殊模式进行前缀分流处理
    ///
    /// 若输入匹配特殊模式且对应功能已启用，则返回生成的候选词列表；否则返回 `None` 继续交由拼音主引擎。
    pub fn try_dispatch(raw_input: &str, config: &AlgoConfig) -> Option<Vec<AlgoCandidate>> {
        if raw_input.is_empty() {
            return None;
        }

        let first_char = raw_input.chars().next()?;
        let body = &raw_input[first_char.len_utf8()..];

        match first_char {
            'v' if config.enable_v_mode => {
                let res = Self::eval_v_mode(body);
                Some(res)
            }
            'u' if config.enable_u_mode && !body.is_empty() => {
                let res = Self::eval_u_mode(body);
                if res.is_empty() {
                    None
                } else {
                    Some(res)
                }
            }
            'i' if config.enable_i_mode && !body.is_empty() => {
                let res = Self::eval_i_mode(body);
                if res.is_empty() {
                    None
                } else {
                    Some(res)
                }
            }
            _ => None,
        }
    }

    /// V 模式：财务金额大写、算术表达式求值、日期时间与便捷工具箱
    fn eval_v_mode(expr: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        let now = chrono::Local::now();

        // 1. 若只输入了 "v"，提供快捷功能入口引导
        if expr.is_empty() {
            let today = now.format("%Y年%m月%d日").to_string();
            let time_str = now.format("%H:%M:%S").to_string();
            candidates.push(AlgoCandidate::new(today, "v", 99_000).with_comment("当前日期"));
            candidates.push(AlgoCandidate::new(time_str, "v", 95_000).with_comment("当前时间"));
            candidates
                .push(AlgoCandidate::new("输入算式 如: v20*5", "v", 90_000).with_comment("计算器"));
            candidates.push(
                AlgoCandidate::new("输入数字 如: v12345", "v", 85_000).with_comment("大写金额"),
            );
            return candidates;
        }

        let expr_lower = expr.to_lowercase();
        // 2. 日期与时间快捷指令（如 vsd, vsj, vrq, vtime, vdate, vnow, vxq）
        if matches!(
            expr_lower.as_str(),
            "sd" | "sj" | "rq" | "time" | "date" | "now" | "xq"
        ) {
            let date_list = DateTimeEvaluator::format_date_formats(now);
            let time_list = DateTimeEvaluator::format_time_formats(now);
            let dt_list = DateTimeEvaluator::format_datetime_formats(now);

            match expr_lower.as_str() {
                "sd" | "now" => {
                    for (idx, dt) in dt_list.into_iter().enumerate() {
                        candidates.push(
                            AlgoCandidate::new(
                                dt,
                                expr,
                                99_000u32.saturating_sub((idx as u32).saturating_mul(100)),
                            )
                            .with_comment("日期时间"),
                        );
                    }
                    for (idx, d) in date_list.into_iter().enumerate() {
                        candidates.push(
                            AlgoCandidate::new(
                                d,
                                expr,
                                90_000u32.saturating_sub((idx as u32).saturating_mul(100)),
                            )
                            .with_comment("当前日期"),
                        );
                    }
                }
                "rq" | "date" => {
                    for (idx, d) in date_list.into_iter().enumerate() {
                        candidates.push(
                            AlgoCandidate::new(
                                d,
                                expr,
                                99_000u32.saturating_sub((idx as u32).saturating_mul(100)),
                            )
                            .with_comment("当前日期"),
                        );
                    }
                }
                "sj" | "time" => {
                    for (idx, t) in time_list.into_iter().enumerate() {
                        candidates.push(
                            AlgoCandidate::new(
                                t,
                                expr,
                                99_000u32.saturating_sub((idx as u32).saturating_mul(100)),
                            )
                            .with_comment("当前时间"),
                        );
                    }
                }
                "xq" => {
                    for (idx, w) in DateTimeEvaluator::format_weekday_formats(now)
                        .into_iter()
                        .enumerate()
                    {
                        candidates.push(
                            AlgoCandidate::new(
                                w,
                                expr,
                                99_000u32.saturating_sub((idx as u32).saturating_mul(100)),
                            )
                            .with_comment("星期"),
                        );
                    }
                }
                _ => {}
            }
            if !candidates.is_empty() {
                return candidates;
            }
        }

        // 3. 优先尝试财务金额大写转换（纯数字及小数，如 v12345, v12345.67）
        if let Some(fin_cands) = FinancialEvaluator::try_evaluate(expr) {
            candidates.extend(fin_cands);
        }

        // 4. 算术表达式安全求值（如 v20*5, v100+20）
        if let Some(val) = Self::eval_math_simple(expr) {
            let effective_val = if val.abs() < 1e-9 { 0.0 } else { val };
            let val_str = if (effective_val.fract()).abs() < 1e-9
                && effective_val.abs() < 9_007_199_254_740_992.0
            {
                let round_int = effective_val.round() as i64;
                if round_int == 0 {
                    "0".to_string()
                } else {
                    format!("{}", round_int)
                }
            } else if (effective_val.fract()).abs() < 1e-9 {
                format!("{:.0}", effective_val)
            } else {
                format!("{:.4}", effective_val)
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            };

            // 避免与已有纯数字候选重复
            if !candidates.iter().any(|c| c.text == val_str) {
                candidates.push(
                    AlgoCandidate::new(val_str.clone(), expr, 85_000).with_comment("计算结果"),
                );
            }
            if expr.contains(|c| "+-*/%xX".contains(c)) {
                candidates.push(
                    AlgoCandidate::new(format!("{}={}", expr, val_str), expr, 80_000)
                        .with_comment("完整算式"),
                );
            }
        }

        // 5. 兜底友好引导
        if candidates.is_empty() {
            candidates.push(
                AlgoCandidate::new(format!("v{}", expr), expr, 50_000)
                    .with_comment("V模式: 输入数字/算式/sd/sj/rq"),
            );
        }

        candidates
    }

    /// 轻量安全四则运算解析器（支持圆括号嵌套优先级、一元正负号与除零保护）
    fn eval_math_simple(expr: &str) -> Option<f64> {
        let trimmed = expr.trim_end_matches('=');
        let chars: Vec<char> = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
        if chars.is_empty() {
            return None;
        }

        let mut pos = 0;
        let val = Self::parse_expr(&chars, &mut pos, 0)?;
        if pos == chars.len() && val.is_finite() {
            let normalized = if val.abs() < 1e-9 { 0.0 } else { val };
            Some(normalized)
        } else {
            None
        }
    }

    /// 解析加减级表达式：term { ('+' | '-') term }
    fn parse_expr(chars: &[char], pos: &mut usize, depth: usize) -> Option<f64> {
        if depth > 32 {
            return None;
        }
        let mut lhs = Self::parse_term(chars, pos, depth)?;
        while let Some(&op) = chars.get(*pos) {
            if op == '+' || op == '-' {
                *pos += 1;
                let rhs = Self::parse_term(chars, pos, depth)?;
                lhs = if op == '+' { lhs + rhs } else { lhs - rhs };
            } else {
                break;
            }
        }
        Some(lhs)
    }

    /// 解析乘除取模级项：factor { ('*' | '/' | '%') factor }
    fn parse_term(chars: &[char], pos: &mut usize, depth: usize) -> Option<f64> {
        let mut lhs = Self::parse_factor(chars, pos, depth)?;
        while let Some(&op) = chars.get(*pos) {
            if op == '*' || op == 'x' || op == 'X' || op == '/' || op == '%' {
                *pos += 1;
                let rhs = Self::parse_factor(chars, pos, depth)?;
                lhs = match op {
                    '*' | 'x' | 'X' => lhs * rhs,
                    '/' => {
                        if rhs.abs() < 1e-12 {
                            return None;
                        }
                        lhs / rhs
                    }
                    '%' => {
                        if rhs.abs() < 1e-12 {
                            return None;
                        }
                        lhs % rhs
                    }
                    _ => unreachable!(),
                };
            } else {
                break;
            }
        }
        Some(lhs)
    }

    /// 解析原子因子：一元正负号、括号子表达式或浮点数常量
    fn parse_factor(chars: &[char], pos: &mut usize, depth: usize) -> Option<f64> {
        if depth > 32 {
            return None;
        }

        // 解析一元正负号（限制至多允许 2 个连续一元正负号）
        let mut sign = 1.0;
        let mut sign_count = 0;
        while let Some(&c) = chars.get(*pos) {
            if c == '+' {
                *pos += 1;
                sign_count += 1;
            } else if c == '-' {
                *pos += 1;
                sign = -sign;
                sign_count += 1;
            } else {
                break;
            }
            if sign_count > 2 {
                return None;
            }
        }

        let &ch = chars.get(*pos)?;
        if ch == '(' {
            *pos += 1;
            let val = Self::parse_expr(chars, pos, depth + 1)?;
            if chars.get(*pos) == Some(&')') {
                *pos += 1;
                return Some(sign * val);
            }
            return None;
        }

        let start = *pos;
        while let Some(&c) = chars.get(*pos) {
            if c.is_ascii_digit() || c == '.' {
                *pos += 1;
            } else {
                break;
            }
        }
        if *pos == start {
            return None;
        }
        let num_str: String = chars[start..*pos].iter().collect();
        // 拒绝以小数点开头/结尾或含有多个小数点的未完成/非法浮点数（如 "10." 或 ".5"）
        if num_str.starts_with('.') || num_str.ends_with('.') || num_str.matches('.').count() > 1 {
            return None;
        }
        let val = num_str.parse::<f64>().ok()?;
        Some(sign * val)
    }

    /// U 模式：拆字与笔画反查
    fn eval_u_mode(input: &str) -> Vec<AlgoCandidate> {
        crate::dispatcher_symbols::query_u_mode(input)
    }

    /// I 模式：特殊序号与符号输出
    fn eval_i_mode(input: &str) -> Vec<AlgoCandidate> {
        crate::dispatcher_symbols::query_i_mode(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_v_mode_calculator() {
        let config = AlgoConfig::default();
        let result = ModeDispatcher::try_dispatch("v1+2*3", &config).expect("应触发V模式");
        assert_eq!(result[0].text, "7");
        assert_eq!(result[1].text, "1+2*3=7");

        // 圆括号嵌套优先级
        let paren_res = ModeDispatcher::try_dispatch("v(1+2)*3", &config).expect("应支持括号算式");
        assert_eq!(paren_res[0].text, "9");
        assert_eq!(paren_res[1].text, "(1+2)*3=9");

        let nested_res =
            ModeDispatcher::try_dispatch("v100/(2+3)", &config).expect("应支持分母括号算式");
        assert_eq!(nested_res[0].text, "20");

        // 一元负号表达式
        let neg_res = ModeDispatcher::try_dispatch("v-5+10", &config).expect("应支持负号开头算式");
        assert_eq!(neg_res[0].text, "5");

        let div_zero = ModeDispatcher::try_dispatch("v10/0", &config).expect("V模式应拦截");
        assert!(!div_zero.iter().any(|c| c.text.contains('=')));
    }

    #[test]
    fn test_v_mode_financial_amount() {
        let config = AlgoConfig::default();
        let result =
            ModeDispatcher::try_dispatch("v12345.67", &config).expect("应触发V模式金额转换");
        assert_eq!(result[0].text, "壹万贰仟叁佰肆拾伍元陆角柒分");
        assert_eq!(result[0].comment.as_deref(), Some("人民币大写"));
        assert_eq!(result[1].text, "壹万贰仟叁佰肆拾伍点陆柒");
        assert_eq!(result[2].text, "12,345.67");
    }

    #[test]
    fn test_u_mode_chaizi() {
        let config = AlgoConfig::default();
        let result = ModeDispatcher::try_dispatch("uhehe", &config).expect("应触发U模式");
        assert_eq!(result[0].text, "囍");

        // 验证三叠字与左右结构拼字
        let mux3 = ModeDispatcher::try_dispatch("umux3", &config).expect("应支持三叠字拆字");
        assert!(mux3.iter().any(|c| c.text == "森"));

        let yux3 = ModeDispatcher::try_dispatch("uyux3", &config).expect("应支持鱼x3拆字");
        assert!(yux3.iter().any(|c| c.text == "鱻"));

        let riyue = ModeDispatcher::try_dispatch("uriyue", &config).expect("应支持日月明拆字");
        assert!(riyue.iter().any(|c| c.text == "明"));

        // 未命中任何拆字规则时应返回 None，不能返回 Some(vec![]) 拦截拼音
        assert!(ModeDispatcher::try_dispatch("ui", &config).is_none());
        assert!(ModeDispatcher::try_dispatch("uzz", &config).is_none());
    }

    #[test]
    fn test_i_mode_symbols() {
        let config = AlgoConfig::default();
        let result = ModeDispatcher::try_dispatch("i1", &config).expect("应触发I模式");
        assert_eq!(result[0].text, "①");

        let res_10 = ModeDispatcher::try_dispatch("i10", &config).expect("应支持两位数序号");
        assert!(res_10.iter().any(|c| c.text == "⑩"));

        let res_jt = ModeDispatcher::try_dispatch("ijt", &config).expect("应支持箭头符号");
        assert!(res_jt.iter().any(|c| c.text == "→"));

        let res_money = ModeDispatcher::try_dispatch("irmb", &config).expect("应支持货币符号");
        assert!(res_money.iter().any(|c| c.text == "¥"));
    }

    #[test]
    fn test_regular_pinyin_passthrough() {
        let config = AlgoConfig::default();
        assert!(ModeDispatcher::try_dispatch("nihao", &config).is_none());
    }

    #[test]
    fn test_math_eval_boundary_conditions() {
        // 极小负数或相反数相加不得输出 -0
        assert_eq!(ModeDispatcher::eval_math_simple("-0.0000000001"), Some(0.0));
        assert_eq!(ModeDispatcher::eval_math_simple("1-1"), Some(0.0));
        let cands = ModeDispatcher::eval_v_mode("1-1");
        assert!(cands.iter().any(|c| c.text == "0"));
        assert!(!cands.iter().any(|c| c.text == "-0"));

        // 拒绝尾随小数点输入
        assert_eq!(ModeDispatcher::eval_math_simple("10."), None);
        assert_eq!(ModeDispatcher::eval_math_simple(".5"), None);

        // 连续一元符号支持至多 2 个，超过 2 个直接拒绝
        assert_eq!(ModeDispatcher::eval_math_simple("--5"), Some(5.0));
        assert_eq!(ModeDispatcher::eval_math_simple("+-5"), Some(-5.0));
        assert_eq!(ModeDispatcher::eval_math_simple("---5"), None);
    }
}
