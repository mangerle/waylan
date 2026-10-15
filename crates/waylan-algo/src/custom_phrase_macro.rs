//! 用户自定义短语高级动态执行宏扩展模块
//!
//! # 设计原理
//! - **实现初衷**：为自定义短语赋予动态时间戳、格式化日期与系统环境注入能力，
//!   解决静态字符串替换无法生成即时模板（如签名、时间戳、报告标题）的局限。
//! - **核心优势**：
//!   1. 零宏语言解析开销：无 `#` 的纯文本快速路径直通；
//!   2. 声明式安全求值：内建时间与安全环境变量宏，杜绝外部命令盲目执行 (RCE)；
//!   3. 格式灵活兼收并蓄：同时兼容用户友好的 `YYYY-MM-DD` 与标准 strftime 格式。
//! - **代价与局限**：宏求值在候选词检索 `lookup` 时实时触发，计算耗时控制在微秒级。

use chrono::{Datelike, Local};

/// 将用户常用的格式化标记符 (YYYY, MM, DD, HH, mm, ss) 转换为标准 strftime 格式符
fn convert_user_pattern_to_chrono(pat: &str) -> String {
    let mut s = pat.to_string();
    s = s.replace("YYYY", "%Y");
    s = s.replace("YY", "%y");
    s = s.replace("MM", "%m");
    s = s.replace("DD", "%d");
    s = s.replace("HH", "%H");
    s = s.replace("mm", "%M");
    s = s.replace("ss", "%S");
    s
}

/// 获取中文星期名称
fn get_chinese_weekday(weekday: chrono::Weekday) -> &'static str {
    match weekday {
        chrono::Weekday::Mon => "星期一",
        chrono::Weekday::Tue => "星期二",
        chrono::Weekday::Wed => "星期三",
        chrono::Weekday::Thu => "星期四",
        chrono::Weekday::Fri => "星期五",
        chrono::Weekday::Sat => "星期六",
        chrono::Weekday::Sun => "星期日",
    }
}

/// 对单个宏指令进行求值
fn eval_single_macro(
    name: &str,
    arg: Option<&str>,
    now: chrono::DateTime<Local>,
) -> Option<String> {
    match name {
        "date" => {
            let pat = arg.unwrap_or("YYYY-MM-DD");
            let chrono_pat = convert_user_pattern_to_chrono(pat);
            Some(now.format(&chrono_pat).to_string())
        }
        "time" => {
            let pat = arg.unwrap_or("HH:mm:ss");
            let chrono_pat = convert_user_pattern_to_chrono(pat);
            Some(now.format(&chrono_pat).to_string())
        }
        "datetime" => {
            let pat = arg.unwrap_or("YYYY-MM-DD HH:mm:ss");
            let chrono_pat = convert_user_pattern_to_chrono(pat);
            Some(now.format(&chrono_pat).to_string())
        }
        "timestamp" => Some(now.timestamp().to_string()),
        "timestamp_ms" => Some(now.timestamp_millis().to_string()),
        "weekday" | "week" => Some(get_chinese_weekday(now.weekday()).to_string()),
        "year" => Some(format!("{:04}", now.year())),
        "month" => Some(format!("{:02}", now.month())),
        "day" => Some(format!("{:02}", now.day())),
        "env" => {
            if let Some(var_name) = arg {
                Some(std::env::var(var_name.trim()).unwrap_or_default())
            } else {
                Some(String::new())
            }
        }
        _ => None,
    }
}

/// 扫描短语模板并实时展开其中的动态宏指令
///
/// # 语法规则
/// - `#date` 或 `#date(YYYY-MM-DD)`：展开为当前本地日期；
/// - `#time` 或 `#time(HH:mm:ss)`：展开为当前本地时间；
/// - `#datetime` 或 `#datetime(...)`：展开为本地日期时间；
/// - `#timestamp`：当前 Unix 纪元秒级时间戳；
/// - `#timestamp_ms`：当前 Unix 毫秒时间戳；
/// - `#weekday` / `#week`：当前中文星期；
/// - `#year`、`#month`、`#day`：当前年、月、日；
/// - `#env(VAR)`：读取系统环境变量 `VAR`；
/// - `##`：转义为字面量字符 `#`。
pub fn expand_dynamic_macros(template: &str) -> String {
    if !template.contains('#') {
        return template.to_string();
    }

    let now = Local::now();
    let mut result = String::with_capacity(template.len() + 16);
    let chars: Vec<char> = template.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '#' {
            // 转义处理：## 展开为字面量 '#'
            if i + 1 < len && chars[i + 1] == '#' {
                result.push('#');
                i += 2;
                continue;
            }

            // 提取宏名称
            let mut j = i + 1;
            while j < len && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }

            let macro_name: String = chars[i + 1..j].iter().collect();

            // 检查是否有参数括号: #macro(arg)
            let mut arg: Option<String> = None;
            if j < len && chars[j] == '(' {
                let mut k = j + 1;
                while k < len && chars[k] != ')' {
                    k += 1;
                }
                if k < len && chars[k] == ')' {
                    arg = Some(chars[j + 1..k].iter().collect());
                    j = k + 1;
                }
            }

            if let Some(expanded) = eval_single_macro(&macro_name, arg.as_deref(), now) {
                result.push_str(&expanded);
                i = j;
                continue;
            } else {
                result.push('#');
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_dynamic_macros_plain_passthrough() {
        assert_eq!(expand_dynamic_macros("hello world"), "hello world");
        assert_eq!(
            expand_dynamic_macros("mymail@domain.com"),
            "mymail@domain.com"
        );
    }

    #[test]
    fn test_expand_dynamic_date_and_time() {
        let now = Local::now();
        let expected_date = now.format("%Y-%m-%d").to_string();
        let expanded = expand_dynamic_macros("Today is #date");
        assert_eq!(expanded, format!("Today is {}", expected_date));

        let expected_cn_date = now.format("%Y年%m月%d日").to_string();
        let expanded_cn = expand_dynamic_macros("#date(YYYY年MM月DD日)");
        assert_eq!(expanded_cn, expected_cn_date);

        let expanded_time = expand_dynamic_macros("Time: #time(HH:mm)");
        let expected_time = now.format("%H:%M").to_string();
        assert_eq!(expanded_time, format!("Time: {}", expected_time));
    }

    #[test]
    fn test_expand_dynamic_timestamp_and_year() {
        let now = Local::now();
        let expanded = expand_dynamic_macros("ts:#timestamp,year:#year");
        assert!(expanded.starts_with("ts:"));
        assert!(expanded.contains(&format!("year:{:04}", now.year())));
    }

    #[test]
    fn test_expand_dynamic_env_and_escape() {
        std::env::set_var("WAYLAN_TEST_ENV_VAR", "micro_wave");
        let expanded = expand_dynamic_macros("Env: #env(WAYLAN_TEST_ENV_VAR), Escaped: ##tag");
        assert_eq!(expanded, "Env: micro_wave, Escaped: #tag");
    }
}
