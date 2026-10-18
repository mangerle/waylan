//! 命令行参数解析与词库管理工具分发模块
//!
//! # 设计原理
//! - **实现初衷**：消除手工比对 `args[1]` 的脆弱命令行判断，提供类型安全、语义清晰的强类型命令与选项抽象。
//! - **核心优势**：支持选项自由组合（如 `--ibus --check`）、健壮的参数校验与精准的中文语法帮助，杜绝索引越界与静默失败。
//! - **代价与局限**：采用轻量手写状态机而非庞大的第三方 CLI 解析库，保持守护进程极速冷启动与微量依赖包体积。

use crate::error::DaemonError;

pub mod dict_cmd;
pub mod self_check;
pub use dict_cmd::DictSubcommand;
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
    /// 词库管理子命令 (dict <子命令>)
    Dict(DictSubcommand),
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
            "dict" => parse_dict_subcommand(&tokens[1..]).map(Self::Dict),
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

/// 解析词库管理子命令参数
fn parse_dict_subcommand(sub_tokens: &[String]) -> Result<DictSubcommand, DaemonError> {
    if sub_tokens.is_empty() || sub_tokens[0] == "-h" || sub_tokens[0] == "--help" {
        return Ok(DictSubcommand::Help);
    }

    match sub_tokens[0].as_str() {
        "build" => {
            let args = &sub_tokens[1..];
            if args.len() < 2 {
                return Err(DaemonError::Cli(
                    "dict build 命令需要至少一个输入文本词库和一个输出路径".into(),
                ));
            }
            let output = args[args.len() - 1].clone();
            let inputs = args[..args.len() - 1].to_vec();
            Ok(DictSubcommand::Build { inputs, output })
        }
        "build-opencc" => {
            let args = &sub_tokens[1..];
            let (chars, phrases, output) = if args.is_empty() {
                (
                    "/usr/share/opencc/STCharacters.txt".to_string(),
                    "/usr/share/opencc/STPhrases.txt".to_string(),
                    "opencc.dict.bin".to_string(),
                )
            } else if args.len() == 3 {
                (args[0].clone(), args[1].clone(), args[2].clone())
            } else {
                return Err(DaemonError::Cli(
                    "dict build-opencc 命令需要指定 [STCharacters.txt] [STPhrases.txt] [输出文件] 三个参数，或完全留空使用系统默认路径".into(),
                ));
            };
            Ok(DictSubcommand::BuildOpenCc {
                chars,
                phrases,
                output,
            })
        }
        "build-aux" => {
            let args = &sub_tokens[1..];
            if args.len() < 3 {
                return Err(DaemonError::Cli(
                    "dict build-aux 命令需要 <英文词表> <符号表> <输出文件> 三个参数".into(),
                ));
            }
            Ok(DictSubcommand::BuildAux {
                english: args[0].clone(),
                symbols: args[1].clone(),
                output: args[2].clone(),
            })
        }
        "import-user" => {
            let args = &sub_tokens[1..];
            if args.len() < 2 {
                return Err(DaemonError::Cli(
                    "dict import-user 命令需要 <用户文本词库> <用户二进制WAL路径> 两个参数".into(),
                ));
            }
            Ok(DictSubcommand::ImportUser {
                input: args[0].clone(),
                user_dict: args[1].clone(),
            })
        }
        other => Err(DaemonError::Cli(format!(
            "未知的词库管理子命令: '{}'，请使用 waylan dict --help 查看使用说明",
            other
        ))),
    }
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
    println!("  dict <子命令>        词库编译与用户词库导入管理工具");
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
    fn test_parse_dict_subcommands() {
        let cmd_build = CliCommand::parse(&to_args(&[
            "waylan", "dict", "build", "a.txt", "b.txt", "out.bin",
        ]))
        .expect("dict build 解析成功");
        assert_eq!(
            cmd_build,
            CliCommand::Dict(DictSubcommand::Build {
                inputs: vec!["a.txt".to_string(), "b.txt".to_string()],
                output: "out.bin".to_string(),
            })
        );

        let cmd_opencc_default = CliCommand::parse(&to_args(&["waylan", "dict", "build-opencc"]))
            .expect("dict build-opencc 默认路径解析成功");
        assert_eq!(
            cmd_opencc_default,
            CliCommand::Dict(DictSubcommand::BuildOpenCc {
                chars: "/usr/share/opencc/STCharacters.txt".to_string(),
                phrases: "/usr/share/opencc/STPhrases.txt".to_string(),
                output: "opencc.dict.bin".to_string(),
            })
        );

        let cmd_aux = CliCommand::parse(&to_args(&[
            "waylan",
            "dict",
            "build-aux",
            "en.txt",
            "sym.txt",
            "aux.bin",
        ]))
        .expect("dict build-aux 解析成功");
        assert_eq!(
            cmd_aux,
            CliCommand::Dict(DictSubcommand::BuildAux {
                english: "en.txt".to_string(),
                symbols: "sym.txt".to_string(),
                output: "aux.bin".to_string(),
            })
        );

        let cmd_import = CliCommand::parse(&to_args(&[
            "waylan",
            "dict",
            "import-user",
            "u.txt",
            "user.bin",
        ]))
        .expect("dict import-user 解析成功");
        assert_eq!(
            cmd_import,
            CliCommand::Dict(DictSubcommand::ImportUser {
                input: "u.txt".to_string(),
                user_dict: "user.bin".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_invalid_args_returns_error() {
        let err = CliCommand::parse(&to_args(&["waylan", "--nonexistent-arg"]))
            .expect_err("未知参数应报错");
        assert!(matches!(err, DaemonError::Cli(_)));

        let err_dict = CliCommand::parse(&to_args(&["waylan", "dict", "build"]))
            .expect_err("缺少必要参数应报错");
        assert!(matches!(err_dict, DaemonError::Cli(_)));
    }
}
