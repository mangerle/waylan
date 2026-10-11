use crate::evaluator::FinancialEvaluator;
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
            'v' if config.enable_v_mode
                && !body.is_empty()
                && body.starts_with(|c: char| c.is_ascii_digit() || c == '(' || c == '-') =>
            {
                let res = Self::eval_v_mode(body);
                if res.is_empty() {
                    None
                } else {
                    Some(res)
                }
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

    /// V 模式：财务金额大写与算术表达式求值
    fn eval_v_mode(expr: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();

        // 1. 优先尝试财务金额大写转换（纯数字及小数，如 v12345, v12345.67）
        if let Some(fin_cands) = FinancialEvaluator::try_evaluate(expr) {
            candidates.extend(fin_cands);
        }

        // 2. 算术表达式安全求值（如 v20*5, v100+20）
        if let Some(val) = Self::eval_math_simple(expr) {
            let val_str = if (val.fract()).abs() < 1e-9 && val.abs() < 9_007_199_254_740_992.0 {
                format!("{}", val.round() as i64)
            } else if (val.fract()).abs() < 1e-9 {
                format!("{:.0}", val)
            } else {
                format!("{:.4}", val)
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
            if expr.contains(|c| "+-*/%".contains(c)) {
                candidates.push(
                    AlgoCandidate::new(format!("{}={}", expr, val_str), expr, 80_000)
                        .with_comment("完整算式"),
                );
            }
        }
        candidates
    }

    /// 轻量安全四则运算解析器（支持圆括号嵌套优先级、一元正负号与除零保护）
    fn eval_math_simple(expr: &str) -> Option<f64> {
        let chars: Vec<char> = expr.chars().filter(|c| !c.is_whitespace()).collect();
        if chars.is_empty() {
            return None;
        }

        let mut pos = 0;
        let val = Self::parse_expr(&chars, &mut pos, 0)?;
        if pos == chars.len() && val.is_finite() {
            Some(val)
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
            if op == '*' || op == '/' || op == '%' {
                *pos += 1;
                let rhs = Self::parse_factor(chars, pos, depth)?;
                lhs = match op {
                    '*' => lhs * rhs,
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
        let &ch = chars.get(*pos)?;
        if ch == '+' {
            *pos += 1;
            return Self::parse_factor(chars, pos, depth + 1);
        }
        if ch == '-' {
            *pos += 1;
            return Some(-Self::parse_factor(chars, pos, depth + 1)?);
        }
        if ch == '(' {
            *pos += 1;
            let val = Self::parse_expr(chars, pos, depth + 1)?;
            if chars.get(*pos) == Some(&')') {
                *pos += 1;
                return Some(val);
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
        num_str.parse::<f64>().ok()
    }

    /// U 模式：拆字与笔画反查
    fn eval_u_mode(input: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        let table: &[(&str, &str, &str)] = &[
            ("hehe", "囍", "拆字: 喜+喜"),
            ("rrrr", "叕", "拆字: 又x4"),
            ("sun", "昍", "拆字: 日+日"),
            ("mumu", "林", "拆字: 木+木"),
            ("mux3", "森", "拆字: 木x3"),
            ("koukou", "回", "拆字: 口+口"),
            ("koux3", "品", "拆字: 口x3"),
            ("xian", "鱻", "拆字: 鱼x3"),
            ("niu", "犇", "拆字: 牛x3"),
            ("yang", "羴", "拆字: 羊x3"),
            ("huo", "焱", "拆字: 火x3"),
            ("jin", "鑫", "拆字: 金x3"),
            ("tu", "垚", "拆字: 土x3"),
            ("shui", "淼", "拆字: 水x3"),
            ("che", "轰", "拆字: 车x3"),
        ];

        for &(code, text, comment) in table {
            if code == input || code.starts_with(input) {
                candidates.push(
                    AlgoCandidate::new(text, format!("u{}", code), 10_000).with_comment(comment),
                );
            }
        }
        candidates
    }

    /// I 模式：特殊序号与符号输出
    fn eval_i_mode(input: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        match input {
            "1" => {
                candidates.push(AlgoCandidate::new("①", "i1", 10_000).with_comment("带圈序号"));
                candidates.push(AlgoCandidate::new("㈠", "i1", 9_000).with_comment("括号序号"));
                candidates.push(AlgoCandidate::new("壹", "i1", 8_000).with_comment("大写数字"));
                candidates.push(AlgoCandidate::new("第一", "i1", 7_000).with_comment("中文序数"));
            }
            "2" => {
                candidates.push(AlgoCandidate::new("②", "i2", 10_000).with_comment("带圈序号"));
                candidates.push(AlgoCandidate::new("㈡", "i2", 9_000).with_comment("括号序号"));
                candidates.push(AlgoCandidate::new("贰", "i2", 8_000).with_comment("大写数字"));
                candidates.push(AlgoCandidate::new("第二", "i2", 7_000).with_comment("中文序数"));
            }
            "3" => {
                candidates.push(AlgoCandidate::new("③", "i3", 10_000).with_comment("带圈序号"));
                candidates.push(AlgoCandidate::new("㈢", "i3", 9_000).with_comment("括号序号"));
                candidates.push(AlgoCandidate::new("叁", "i3", 8_000).with_comment("大写数字"));
                candidates.push(AlgoCandidate::new("第三", "i3", 7_000).with_comment("中文序数"));
            }
            "+" => {
                candidates.push(AlgoCandidate::new("±", "i+", 10_000).with_comment("正负号"));
                candidates
                    .push(AlgoCandidate::new("\u{2795}", "i+", 9_000).with_comment("运算符号"));
            }
            "*" => {
                candidates.push(AlgoCandidate::new("×", "i*", 10_000).with_comment("乘号"));
                candidates.push(AlgoCandidate::new("★", "i*", 9_000).with_comment("五角星"));
                candidates.push(AlgoCandidate::new("☆", "i*", 8_000).with_comment("空心星"));
            }
            _ => {}
        }
        candidates
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

        let div_zero = ModeDispatcher::try_dispatch("v10/0", &config);
        assert!(div_zero.is_none());
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

        // 未命中任何拆字规则时应返回 None，不能返回 Some(vec![]) 拦截拼音
        assert!(ModeDispatcher::try_dispatch("ui", &config).is_none());
        assert!(ModeDispatcher::try_dispatch("uzz", &config).is_none());
    }

    #[test]
    fn test_i_mode_symbols() {
        let config = AlgoConfig::default();
        let result = ModeDispatcher::try_dispatch("i1", &config).expect("应触发I模式");
        assert_eq!(result[0].text, "①");
    }

    #[test]
    fn test_regular_pinyin_passthrough() {
        let config = AlgoConfig::default();
        assert!(ModeDispatcher::try_dispatch("nihao", &config).is_none());
    }
}
