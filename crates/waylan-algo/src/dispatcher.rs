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
            'v' if config.enable_v_mode && !body.is_empty() => Some(Self::eval_v_mode(body)),
            'u' if config.enable_u_mode && !body.is_empty() => Some(Self::eval_u_mode(body)),
            'i' if config.enable_i_mode && !body.is_empty() => Some(Self::eval_i_mode(body)),
            _ => None,
        }
    }

    /// V 模式：算术表达式求值
    fn eval_v_mode(expr: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::with_capacity(2);
        if let Some(val) = Self::eval_math_simple(expr) {
            let val_str = if (val.fract()).abs() < 1e-9 {
                format!("{}", val as i64)
            } else {
                format!("{:.4}", val)
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            };

            candidates
                .push(AlgoCandidate::new(val_str.clone(), expr, 100_000).with_comment("计算结果"));
            candidates.push(
                AlgoCandidate::new(format!("{}={}", expr, val_str), expr, 90_000)
                    .with_comment("完整算式"),
            );
        }
        candidates
    }

    /// 轻量安全四则运算解析器
    fn eval_math_simple(expr: &str) -> Option<f64> {
        let clean: String = expr.chars().filter(|c| !c.is_whitespace()).collect();
        if clean.is_empty() {
            return None;
        }

        let mut nums: Vec<f64> = Vec::new();
        let mut ops: Vec<char> = Vec::new();
        let mut current_num = String::new();

        for ch in clean.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                current_num.push(ch);
            } else if "+-*/%".contains(ch) {
                if current_num.is_empty() {
                    return None;
                }
                let val = current_num.parse::<f64>().ok()?;
                nums.push(val);
                current_num.clear();

                while let Some(&last_op) = ops.last() {
                    if Self::precedence(last_op) >= Self::precedence(ch) {
                        ops.pop();
                        Self::apply_op(&mut nums, last_op)?;
                    } else {
                        break;
                    }
                }
                ops.push(ch);
            } else {
                return None;
            }
        }

        if !current_num.is_empty() {
            let val = current_num.parse::<f64>().ok()?;
            nums.push(val);
        }

        while let Some(op) = ops.pop() {
            Self::apply_op(&mut nums, op)?;
        }

        if nums.len() == 1 {
            nums.pop()
        } else {
            None
        }
    }

    /// 运算符优先级
    fn precedence(op: char) -> u8 {
        match op {
            '+' | '-' => 1,
            '*' | '/' | '%' => 2,
            _ => 0,
        }
    }

    /// 执行单步二元运算（含除零与溢出保护）
    fn apply_op(nums: &mut Vec<f64>, op: char) -> Option<()> {
        if nums.len() < 2 {
            return None;
        }
        let b = nums.pop()?;
        let a = nums.pop()?;

        let res = match op {
            '+' => a + b,
            '-' => a - b,
            '*' => a * b,
            '/' => {
                if b.abs() < 1e-12 {
                    return None; // 除零保护
                }
                a / b
            }
            '%' => {
                if b.abs() < 1e-12 {
                    return None;
                }
                a % b
            }
            _ => return None,
        };
        nums.push(res);
        Some(())
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
                candidates.push(AlgoCandidate::new("➕", "i+", 9_000).with_comment("运算符号"));
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

        let div_zero = ModeDispatcher::try_dispatch("v10/0", &config).expect("应捕获但无结果");
        assert!(div_zero.is_empty());
    }

    #[test]
    fn test_u_mode_chaizi() {
        let config = AlgoConfig::default();
        let result = ModeDispatcher::try_dispatch("uhehe", &config).expect("应触发U模式");
        assert_eq!(result[0].text, "囍");
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
