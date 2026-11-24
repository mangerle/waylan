//! 四则运算与数学表达式求值器 (Math Evaluator)
//!
//! # 设计原理
//! - **实现初衷**：在 V 模式下提供轻量、安全、高效的算术表达式计算能力（如 1+2*3、(10+20)/5）。
//! - **核心优势**：
//!   1. 纯手工递归下降语法解析器，零引入大型脚本或 AST 依赖，无运行时开销；
//!   2. 支持加减乘除、取模、中英文乘号（x/X）、圆括号优先级嵌套与一元正负号；
//!   3. 内置除零保护、浮点精度舍入与递归深度限制（防爆栈）。
//! - **代价与局限**：仅支持标准四则混合运算，不支持复杂三角函数或代数方程。

use crate::types::AlgoCandidate;

/// 数学表达式求值器
#[derive(Debug, Default)]
pub struct MathEvaluator;

impl MathEvaluator {
    /// 评估算术表达式并生成对应候选
    pub fn try_evaluate(expr: &str, existing: &[AlgoCandidate]) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        if let Some(val) = Self::eval_math_simple(expr) {
            let effective_val = if val.abs() < 1e-9 { 0.0 } else { val };
            let val_str = format_math_val_str(effective_val);

            // 避免与已有纯数字候选重复
            if !existing.iter().any(|c| c.text == val_str) {
                candidates.push(AlgoCandidate::new(val_str.clone(), expr, 85_000));
            }
            if expr.contains(|c| "+-*/%xX".contains(c)) {
                candidates.push(AlgoCandidate::new(
                    format!("{}={}", expr, val_str),
                    expr,
                    80_000,
                ));
            }
        }
        candidates
    }

    /// 轻量安全四则运算解析器（支持圆括号嵌套优先级、一元正负号与除零保护）
    pub fn eval_math_simple(expr: &str) -> Option<f64> {
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
                    // 外层守卫已将运算符限定为 '*' | 'x' | 'X' | '/' | '%'，
                    // 此处仅作兜底，按解析失败处理而非 panic，避免异常输入击穿求值管线
                    _ => return None,
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

        let sign = Self::parse_unary_sign(chars, pos)?;

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

        let val = Self::parse_number_literal(chars, pos)?;
        Some(sign * val)
    }

    /// 解析原子因子前的一元正负号序列，返回符号值（正负号超过两个视为非法输入）
    fn parse_unary_sign(chars: &[char], pos: &mut usize) -> Option<f64> {
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
        Some(sign)
    }

    /// 解析原子因子中的浮点数常量文本，拒绝前导点、尾随点与多点格式
    fn parse_number_literal(chars: &[char], pos: &mut usize) -> Option<f64> {
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
        if num_str.starts_with('.') || num_str.ends_with('.') || num_str.matches('.').count() > 1 {
            return None;
        }
        let val = num_str.parse::<f64>().ok()?;
        Some(val)
    }
}

/// 格式化浮点数值为紧凑显示字符串
pub fn format_math_val_str(effective_val: f64) -> String {
    if effective_val.fract().abs() < 1e-9 && effective_val.abs() < 9_007_199_254_740_992.0 {
        let round_int = effective_val.round() as i64;
        if round_int == 0 {
            "0".to_string()
        } else {
            format!("{}", round_int)
        }
    } else if effective_val.fract().abs() < 1e-9 {
        format!("{:.0}", effective_val)
    } else {
        format!("{:.4}", effective_val)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_math_evaluator_basic() {
        assert_eq!(MathEvaluator::eval_math_simple("1+2*3"), Some(7.0));
        assert_eq!(MathEvaluator::eval_math_simple("(1+2)*3"), Some(9.0));
        assert_eq!(MathEvaluator::eval_math_simple("10/2"), Some(5.0));
        assert_eq!(MathEvaluator::eval_math_simple("10/0"), None);
        assert_eq!(MathEvaluator::eval_math_simple("-5+3"), Some(-2.0));
    }

    #[test]
    fn test_math_evaluator_candidates() {
        let cands = MathEvaluator::try_evaluate("1+2*3", &[]);
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].text, "7");
        assert_eq!(cands[1].text, "1+2*3=7");
    }
}
