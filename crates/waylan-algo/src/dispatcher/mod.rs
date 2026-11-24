use crate::evaluator::{DateTimeEvaluator, FinancialEvaluator, MathEvaluator};
use crate::types::{AlgoCandidate, AlgoConfig};

pub mod symbols;

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
                if res.is_empty() { None } else { Some(res) }
            }
            'i' if config.enable_i_mode && !body.is_empty() => {
                let res = Self::eval_i_mode(body);
                if res.is_empty() { None } else { Some(res) }
            }
            _ => None,
        }
    }

    /// V 模式：财务金额大写、算术表达式求值、日期时间与便捷工具箱
    fn eval_v_mode(expr: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();

        // 1. 日期与时间快捷指令
        if let Some(dt_cands) = Self::eval_v_datetime_shortcuts(expr) {
            return dt_cands;
        }

        // 2. 财务金额大写转换（纯数字及小数，如 v12345, v12345.67）
        if let Some(fin_cands) = FinancialEvaluator::try_evaluate(expr) {
            candidates.extend(fin_cands);
        }

        // 3. 算术表达式安全求值（如 v20*5, v100+20）
        candidates.extend(MathEvaluator::try_evaluate(expr, &candidates));

        // 4. 兜底直通候选（纯净直出输入原始内容，不附加任何内部教学说明）
        if candidates.is_empty() {
            candidates.push(AlgoCandidate::new(format!("v{}", expr), expr, 50_000));
        }

        candidates
    }

    /// 评估 V 模式下的日期时间快捷指令（如 vsd, vsj, vrq, vtime 等）
    fn eval_v_datetime_shortcuts(expr: &str) -> Option<Vec<AlgoCandidate>> {
        if expr.is_empty() {
            let now = chrono::Local::now();
            let today = now.format("%Y年%m月%d日").to_string();
            let time_str = now.format("%H:%M:%S").to_string();
            return Some(vec![
                AlgoCandidate::new(today, "v", 99_000),
                AlgoCandidate::new(time_str, "v", 95_000),
            ]);
        }

        let expr_lower = expr.to_lowercase();
        let now = chrono::Local::now();
        let mut candidates = Vec::new();

        // 声明式分组表：每个快捷指令对应若干组「候选条目 -> 基准分值」，
        // 由同一条阶梯降序追加逻辑统一消费，新增指令仅需补一行映射。
        let groups: Vec<(Vec<String>, u32)> = match expr_lower.as_str() {
            "sd" | "now" => vec![
                (DateTimeEvaluator::format_datetime_formats(now), 99_000),
                (DateTimeEvaluator::format_date_formats(now), 90_000),
            ],
            "rq" | "date" => vec![(DateTimeEvaluator::format_date_formats(now), 99_000)],
            "sj" | "time" => vec![(DateTimeEvaluator::format_time_formats(now), 99_000)],
            "xq" => vec![(DateTimeEvaluator::format_weekday_formats(now), 99_000)],
            _ => return None,
        };

        for (items, base_score) in groups {
            append_staggered_candidates(
                &mut candidates,
                StaggeredCandidateParams {
                    items,
                    expr,
                    base_score,
                },
            );
        }

        if candidates.is_empty() {
            None
        } else {
            Some(candidates)
        }
    }

    /// U 模式：拆字与笔画反查
    fn eval_u_mode(input: &str) -> Vec<AlgoCandidate> {
        symbols::query_u_mode(input)
    }

    /// I 模式：特殊序号与符号输出
    fn eval_i_mode(input: &str) -> Vec<AlgoCandidate> {
        symbols::query_i_mode(input)
    }
}

/// 阶梯降序候选追加参数
///
/// # 设计原理
/// - **实现初衷**：日期时间类候选由"条目列表 + 触发表达式 + 基准分值 + 备注"四要素共同决定，
///   平铺传参易发生实参与形参错位。
/// - **核心优势**：以参数对象聚合同源要素，调用点按名传参即可完成语义绑定。
/// - **代价与局限**：多引入一个结构体类型。
struct StaggeredCandidateParams<'a> {
    /// 待追加的候选文本条目
    items: Vec<String>,
    /// 触发本次查询的原始表达式
    expr: &'a str,
    /// 首个条目的基准分值，后续条目按 100 分逐级递减
    base_score: u32,
}

/// 将文本条目按分值阶梯递减包装为算法候选列表
fn append_staggered_candidates(
    candidates: &mut Vec<AlgoCandidate>,
    params: StaggeredCandidateParams<'_>,
) {
    for (idx, item) in params.items.into_iter().enumerate() {
        let score = params
            .base_score
            .saturating_sub((idx as u32).saturating_mul(100));
        candidates.push(AlgoCandidate::new(item, params.expr, score));
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
    fn test_v_mode_fallback_passthrough_no_comment() {
        let config = AlgoConfig::default();
        let res = ModeDispatcher::try_dispatch("v1.5.1", &config).expect("v开头应直通处理");
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].text, "v1.5.1");
        assert_eq!(
            res[0].comment, None,
            "版本号等兜底直通候选绝不应包含任何内部教学提示"
        );
    }

    #[test]
    fn test_v_mode_financial_amount() {
        let config = AlgoConfig::default();
        let result =
            ModeDispatcher::try_dispatch("v12345.67", &config).expect("应触发V模式金额转换");
        assert_eq!(result[0].text, "壹万贰仟叁佰肆拾伍元陆角柒分");
        assert_eq!(result[0].comment.as_deref(), None);
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

        let uh = ModeDispatcher::try_dispatch("uh", &config).expect("应支持单笔画h拆字");
        assert_eq!(uh[0].text, "一", "精确匹配的单笔画横应排在首位");

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
        assert_eq!(MathEvaluator::eval_math_simple("-0.0000000001"), Some(0.0));
        assert_eq!(MathEvaluator::eval_math_simple("1-1"), Some(0.0));
        let cands = ModeDispatcher::eval_v_mode("1-1");
        assert!(cands.iter().any(|c| c.text == "0"));
        assert!(!cands.iter().any(|c| c.text == "-0"));

        // 拒绝尾随小数点输入
        assert_eq!(MathEvaluator::eval_math_simple("10."), None);
        assert_eq!(MathEvaluator::eval_math_simple(".5"), None);

        // 连续一元符号支持至多 2 个，超过 2 个直接拒绝
        assert_eq!(MathEvaluator::eval_math_simple("--5"), Some(5.0));
        assert_eq!(MathEvaluator::eval_math_simple("+-5"), Some(-5.0));
        assert_eq!(MathEvaluator::eval_math_simple("---5"), None);
    }
}
