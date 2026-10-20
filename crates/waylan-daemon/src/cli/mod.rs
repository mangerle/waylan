//! 守护进程命令行参数解析与子命令分发模块
//!
//! # 设计原理
//! - **实现初衷**：消除手工比对 `args[1]` 的脆弱命令行判断，提供类型安全、语义清晰的强类型命令与选项抽象。
//! - **核心优势**：支持选项自由组合（如 `--ibus --check`）、健壮的参数校验与精准的中文语法帮助，杜绝索引越界与静默失败。
//! - **代价与局限**：采用轻量手写状态机而非庞大的第三方 CLI 解析库，保持守护进程极速冷启动与微量依赖包体积。

use crate::error::DaemonError;

pub mod self_check;
pub use self_check::run_self_check;

/// 顶层命令行解析决策枚举
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    /// 显示版本信息 (-V, --version, -v)
    Version,
    /// 打印主程序使用帮助 (-h, --help, help)
    Help,
    /// 启动图形化偏好设置中心 (settings, --settings)
    Settings,
    /// 输出符合 IBus 引擎规范的 XML 描述 (--xml, --ibus-xml)
    IbusEnginesXml,
    /// 输出符合 IBus 组件元数据规范的 XML 描述 (--ibus-component-xml, --component-xml)
    IbusComponentXml,
    /// 守护进程运行模式
    Daemon(DaemonOptions),
}

/// 守护进程运行选项参数包
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DaemonOptions {
    /// 以 IBus 引擎后端模式启动（适配 GNOME / Mutter）
    pub ibus_backend: bool,
    /// 启动前执行核心算法与词库完整性自检
    pub self_check: bool,
}

impl CliCommand {
    /// 解析命令行传入的原始参数列表
    ///
    /// # Errors
    /// 当用户输入非法参数或缺少必要入参时返回 `DaemonError::Cli`。
    pub fn parse(args: &[String]) -> Result<Self, DaemonError> {
        let tokens = extract_cli_tokens(args);
        if tokens.is_empty() {
            return Ok(Self::Daemon(DaemonOptions::default()));
        }

        match tokens[0].as_str() {
            "-V" | "--version" | "-v" => Ok(Self::Version),
            "-h" | "--help" | "help" => Ok(Self::Help),
            "settings" | "--settings" => Ok(Self::Settings),
            "--xml" | "--ibus-xml" => Ok(Self::IbusEnginesXml),
            "--ibus-component-xml" | "--component-xml" => Ok(Self::IbusComponentXml),
            _ => parse_daemon_options(tokens).map(Self::Daemon),
        }
    }
}

/// 从原始命令行入参中滤除二进制可执行程序名自身的路径前缀
fn extract_cli_tokens(args: &[String]) -> &[String] {
    if let Some((first, rest)) = args.split_first() {
        if !first.starts_with('-')
            && (first.ends_with("waylan") || first.contains('/') || first == "cargo")
        {
            return rest;
        }
    }
    args
}

/// 解析守护进程组合运行选项
fn parse_daemon_options(tokens: &[String]) -> Result<DaemonOptions, DaemonError> {
    let mut opts = DaemonOptions::default();
    for token in tokens {
        match token.as_str() {
            "--ibus" | "ibus" => opts.ibus_backend = true,
            "--check" => opts.self_check = true,
            other => {
                return Err(DaemonError::Cli(format!(
                    "未知的命令行参数: '{}'，请使用 --help 查看使用说明",
                    other
                )));
            }
        }
    }
    Ok(opts)
}

/// 打印主程序 CLI 使用帮助
pub fn print_main_usage() {
    println!("微澜输入法 (WayLan) - 专为 Wayland 打造的现代轻量纯 Rust 中文输入法");
    println!("用法: waylan [选项 | 子命令]");
    println!("选项:");
    println!("  -h, --help           显示本帮助信息");
    println!("  -V, --version        显示版本信息");
    println!("  --check              执行算法引擎与词库环境自检");
    println!(
        "  --ibus               以 IBus 引擎后端模式启动（适配 Ubuntu GNOME / Mutter 等桌面）"
    );
    println!("  --xml                输出符合 IBus 引擎规范的 <engines> XML 描述");
    println!("  --ibus-component-xml 输出符合 IBus 组件元数据规范的 <component> XML 描述");
    println!("  --settings           启动图形化偏好设置界面");
    println!("子命令:");
    println!("  settings             启动图形化偏好设置界面");
    println!("词库编译与导入请使用独立管理工具: waylan-dict --help");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_parse_default_daemon() {
        let cmd = CliCommand::parse(&to_args(&["waylan"])).expect("默认启动无参数解析成功");
        assert_eq!(cmd, CliCommand::Daemon(DaemonOptions::default()));

        let cmd_empty = CliCommand::parse(&[]).expect("空切片参数解析成功");
        assert_eq!(cmd_empty, CliCommand::Daemon(DaemonOptions::default()));
    }

    #[test]
    fn test_parse_flags_combination() {
        let cmd_ibus = CliCommand::parse(&to_args(&["waylan", "--ibus"])).expect("--ibus 解析成功");
        assert_eq!(
            cmd_ibus,
            CliCommand::Daemon(DaemonOptions {
                ibus_backend: true,
                self_check: false,
            })
        );

        let cmd_check =
            CliCommand::parse(&to_args(&["waylan", "--check"])).expect("--check 解析成功");
        assert_eq!(
            cmd_check,
            CliCommand::Daemon(DaemonOptions {
                ibus_backend: false,
                self_check: true,
            })
        );

        let cmd_both = CliCommand::parse(&to_args(&["waylan", "--ibus", "--check"]))
            .expect("组合标志解析成功");
        assert_eq!(
            cmd_both,
            CliCommand::Daemon(DaemonOptions {
                ibus_backend: true,
                self_check: true,
            })
        );
    }

    #[test]
    fn test_parse_early_commands() {
        assert_eq!(
            CliCommand::parse(&to_args(&["waylan", "--version"])).unwrap(),
            CliCommand::Version
        );
        assert_eq!(
            CliCommand::parse(&to_args(&["waylan", "-V"])).unwrap(),
            CliCommand::Version
        );
        assert_eq!(
            CliCommand::parse(&to_args(&["waylan", "--help"])).unwrap(),
            CliCommand::Help
        );
        assert_eq!(
            CliCommand::parse(&to_args(&["waylan", "settings"])).unwrap(),
            CliCommand::Settings
        );
        assert_eq!(
            CliCommand::parse(&to_args(&["waylan", "--xml"])).unwrap(),
            CliCommand::IbusEnginesXml
        );
        assert_eq!(
            CliCommand::parse(&to_args(&["waylan", "--component-xml"])).unwrap(),
            CliCommand::IbusComponentXml
        );
    }

    #[test]
    fn test_parse_invalid_args_returns_error() {
        let err = CliCommand::parse(&to_args(&["waylan", "--nonexistent-arg"]))
            .expect_err("未知参数应报错");
        assert!(matches!(err, DaemonError::Cli(_)));
    }
}
