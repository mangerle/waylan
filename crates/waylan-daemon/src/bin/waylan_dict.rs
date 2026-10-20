//! 微澜词库离线构建与用户词库导入工具
//!
//! 独立于常驻守护进程的管理二进制，避免将 DAT/OpenCC 构建器链接进生产输入法进程。
//! 通过 `admin-tools` 特性启用，安装后以 `waylan-dict` 命令调用。

use std::env;
use std::path::Path;
use std::process;

use thiserror::Error;
use waylan_algo::builder::DictBuilder;
use waylan_algo::error::AlgoError;
use waylan_algo::opencc::OpenCcDict;
use waylan_engine::importer::{DictImportError, DictImportService};

/// 词库管理工具错误
#[derive(Debug, Error)]
enum DictToolError {
    /// 命令行用法错误
    #[error("命令行参数错误: {0}")]
    Usage(String),

    /// 词库构建或读写错误
    #[error("词库构建失败: {0}")]
    Algo(#[from] AlgoError),

    /// 用户词库导入错误
    #[error("用户词库导入失败: {0}")]
    Import(#[from] DictImportError),
}

/// 词库管理子命令
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolCommand {
    Help,
    Build {
        inputs: Vec<String>,
        output: String,
    },
    BuildOpenCc {
        chars: String,
        phrases: String,
        output: String,
    },
    BuildAux {
        english: String,
        symbols: String,
        output: String,
    },
    ImportUser {
        input: String,
        user_dict: String,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("错误: {}", e);
        process::exit(2);
    }
}

fn run() -> Result<(), DictToolError> {
    let args: Vec<String> = env::args().collect();
    let command = parse_args(&args)?;
    execute(command)
}

/// 解析命令行参数（首项为可执行文件路径）
fn parse_args(args: &[String]) -> Result<ToolCommand, DictToolError> {
    let tokens = args.get(1..).unwrap_or(&[]);
    if tokens.is_empty() || matches!(tokens[0].as_str(), "-h" | "--help" | "help") {
        return Ok(ToolCommand::Help);
    }

    match tokens[0].as_str() {
        "build" => {
            let args = &tokens[1..];
            if args.len() < 2 {
                return Err(DictToolError::Usage(
                    "dict build 命令需要至少一个输入文本词库和一个输出路径".into(),
                ));
            }
            let output = args[args.len() - 1].clone();
            let inputs = args[..args.len() - 1].to_vec();
            Ok(ToolCommand::Build { inputs, output })
        }
        "build-opencc" => {
            let args = &tokens[1..];
            let (chars, phrases, output) = if args.is_empty() {
                (
                    "/usr/share/opencc/STCharacters.txt".to_string(),
                    "/usr/share/opencc/STPhrases.txt".to_string(),
                    "opencc.dict.bin".to_string(),
                )
            } else if args.len() == 3 {
                (args[0].clone(), args[1].clone(), args[2].clone())
            } else {
                return Err(DictToolError::Usage(
                    "dict build-opencc 命令需要指定 [STCharacters.txt] [STPhrases.txt] [输出文件] 三个参数，或完全留空使用系统默认路径".into(),
                ));
            };
            Ok(ToolCommand::BuildOpenCc {
                chars,
                phrases,
                output,
            })
        }
        "build-aux" => {
            let args = &tokens[1..];
            if args.len() < 3 {
                return Err(DictToolError::Usage(
                    "dict build-aux 命令需要 <英文词表> <符号表> <输出文件> 三个参数".into(),
                ));
            }
            Ok(ToolCommand::BuildAux {
                english: args[0].clone(),
                symbols: args[1].clone(),
                output: args[2].clone(),
            })
        }
        "import-user" => {
            let args = &tokens[1..];
            if args.len() < 2 {
                return Err(DictToolError::Usage(
                    "dict import-user 命令需要 <用户文本词库> <用户二进制WAL路径> 两个参数".into(),
                ));
            }
            Ok(ToolCommand::ImportUser {
                input: args[0].clone(),
                user_dict: args[1].clone(),
            })
        }
        other => Err(DictToolError::Usage(format!(
            "未知的词库管理子命令: '{}'，请使用 waylan-dict --help 查看使用说明",
            other
        ))),
    }
}

/// 执行词库管理子命令
fn execute(command: ToolCommand) -> Result<(), DictToolError> {
    match command {
        ToolCommand::Help => {
            print_usage();
            Ok(())
        }
        ToolCommand::Build { inputs, output } => {
            println!(
                "[词库构建] 正在将文本词库编译为零拷贝 DAT 紧凑二进制词库: {}",
                output
            );
            let input_refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
            let stats = DictBuilder::build_compact_binary_dict(&input_refs, Path::new(&output))?;
            println!(
                "[构建完成] 行数: {}, 有效词条: {}, 键数: {}, 耗时: {} ms",
                stats.total_lines, stats.total_words, stats.total_keys, stats.elapsed_ms
            );
            Ok(())
        }
        ToolCommand::BuildOpenCc {
            chars,
            phrases,
            output,
        } => {
            println!(
                "[简繁词库] 正在将 OpenCC 词表编译为二进制紧凑词库: {} + {} -> {}",
                chars, phrases, output
            );
            let dict = OpenCcDict::from_tsv_files(&chars, &phrases)?;
            dict.save_to_file(&output)?;
            println!(
                "[简繁词库] 编译完成，短语数: {}, 单字数: {}",
                dict.phrases_len(),
                dict.chars.len()
            );
            Ok(())
        }
        ToolCommand::BuildAux {
            english,
            symbols,
            output,
        } => {
            println!(
                "[辅助词库] 编译英文与符号表情词库: {} + {} -> {}",
                english, symbols, output
            );
            let stats = DictBuilder::build_auxiliary_dict(&[&english], &[&symbols], &output)?;
            println!(
                "[辅助词库] 编译完成，英文词汇数: {}, 符号表情数: {}",
                stats.english_words.len(),
                stats.symbols.len()
            );
            Ok(())
        }
        ToolCommand::ImportUser { input, user_dict } => {
            println!(
                "[用户词库] 从外部词库文件导入自造词与调频记录: {} -> {}",
                input, user_dict
            );
            let report = DictImportService::import_pinyin_dict(&input, &user_dict)?;
            println!(
                "[用户词库] 成功导入 {} 条记录，当前词库总词量: {} 条",
                report.imported_count, report.total_entries
            );
            Ok(())
        }
    }
}

/// 打印词库管理工具使用帮助
fn print_usage() {
    println!("微澜输入法 (WayLan) 词库构建管理工具");
    println!("用法: waylan-dict <子命令> [参数...]");
    println!("  build <输入文本词库1> [输入2...] <输出二进制文件>");
    println!("  build-opencc [STCharacters.txt] [STPhrases.txt] [输出文件]");
    println!("  build-aux <英文词表> <符号表> <输出文件>");
    println!("  import-user <外部用户词表.txt> <用户二进制WAL路径>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_parse_build_and_import() {
        assert_eq!(
            parse_args(&args(&[
                "waylan-dict",
                "build",
                "a.txt",
                "b.txt",
                "out.bin"
            ]))
            .unwrap(),
            ToolCommand::Build {
                inputs: vec!["a.txt".to_string(), "b.txt".to_string()],
                output: "out.bin".to_string(),
            }
        );
        assert_eq!(
            parse_args(&args(&["waylan-dict", "import-user", "u.txt", "user.bin"])).unwrap(),
            ToolCommand::ImportUser {
                input: "u.txt".to_string(),
                user_dict: "user.bin".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_errors() {
        assert!(matches!(
            parse_args(&args(&["waylan-dict", "build"])),
            Err(DictToolError::Usage(_))
        ));
        assert!(matches!(
            parse_args(&args(&["waylan-dict", "unknown"])),
            Err(DictToolError::Usage(_))
        ));
    }
}
