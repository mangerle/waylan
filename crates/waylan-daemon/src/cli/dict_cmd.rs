//! 词库编译、转换与用户词库导入管理子命令
//!
//! # 设计原理
//! - **实现初衷**：将繁重且独立的词库预编译（DAT 二进制转换、OpenCC 简繁字典编译、辅助词表构建与用户自造词导入）
//!   从守护进程 CLI 顶层派发中剥离，形成内聚的词库子命令执行单元。
//! - **核心优势**：关注点分离，简化 CLI 入口解析器复杂度，使守护进程各业务链路职责清晰。
//! - **代价与局限**：作为 CLI 执行动作，直接通过标准输出/日志与用户终端交互。

use crate::error::DaemonError;
use log::info;
use std::path::Path;
use waylan_algo::builder::DictBuilder;
use waylan_algo::opencc::OpenCcDict;
use waylan_engine::importer::DictImportService;

/// 词库管理子命令枚举
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictSubcommand {
    /// 打印词库命令帮助信息
    Help,
    /// 编译文本词库为零拷贝 DAT 紧凑二进制词库
    Build { inputs: Vec<String>, output: String },
    /// 编译 OpenCC 简繁转换两级词库
    BuildOpenCc {
        chars: String,
        phrases: String,
        output: String,
    },
    /// 编译英文与符号表情辅助词库
    BuildAux {
        english: String,
        symbols: String,
        output: String,
    },
    /// 从外部文本文件导入用户自造词与调频记录
    ImportUser { input: String, user_dict: String },
}

impl DictSubcommand {
    /// 执行对应的词库编译或管理操作
    ///
    /// # Errors
    /// 当源文件不存在、格式解析异常或写入磁盘失败时返回 `DaemonError`。
    pub fn execute(self) -> Result<(), DaemonError> {
        match self {
            Self::Help => {
                print_dict_usage();
                Ok(())
            }
            Self::Build { inputs, output } => {
                info!(
                    "[词库构建] 正在将文本词库编译为零拷贝 DAT 紧凑二进制词库: {}",
                    output
                );
                let input_refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
                let stats =
                    DictBuilder::build_compact_binary_dict(&input_refs, Path::new(&output))?;
                info!(
                    "[构建完成] 行数: {}, 有效词条: {}, 键数: {}, 耗时: {} ms",
                    stats.total_lines, stats.total_words, stats.total_keys, stats.elapsed_ms
                );
                Ok(())
            }
            Self::BuildOpenCc {
                chars,
                phrases,
                output,
            } => {
                info!(
                    "[简繁词库] 正在将 OpenCC 词表编译为二进制紧凑词库: {} + {} -> {}",
                    chars, phrases, output
                );
                let dict = OpenCcDict::from_tsv_files(&chars, &phrases)?;
                dict.save_to_file(&output)?;
                info!(
                    "[简繁词库] 编译完成，短语数: {}, 单字数: {}",
                    dict.phrases_len(),
                    dict.chars.len()
                );
                Ok(())
            }
            Self::BuildAux {
                english,
                symbols,
                output,
            } => {
                info!(
                    "[辅助词库] 编译英文与符号表情词库: {} + {} -> {}",
                    english, symbols, output
                );
                let stats = DictBuilder::build_auxiliary_dict(&[&english], &[&symbols], &output)?;
                info!(
                    "[辅助词库] 编译完成，英文词汇数: {}, 符号表情数: {}",
                    stats.english_words.len(),
                    stats.symbols.len()
                );
                Ok(())
            }
            Self::ImportUser { input, user_dict } => {
                info!(
                    "[用户词库] 从外部词库文件导入自造词与调频记录: {} -> {}",
                    input, user_dict
                );
                let report = DictImportService::import_pinyin_dict(&input, &user_dict)?;
                info!(
                    "[用户词库] 成功导入 {} 条记录，当前词库总词量: {} 条",
                    report.imported_count, report.total_entries
                );
                Ok(())
            }
        }
    }
}

/// 打印词库管理 CLI 使用帮助
pub fn print_dict_usage() {
    println!("微澜输入法 (WayLan) 词库构建管理工具");
    println!("用法: waylan dict <子命令> [参数...]");
    println!("  build <输入文本词库1> [输入2...] <输出二进制文件>");
    println!("  build-opencc [STCharacters.txt] [STPhrases.txt] [输出文件]");
    println!("  build-aux <英文词表> <符号表> <输出文件>");
    println!("  import-user <外部用户词表.txt> <用户二进制WAL路径>");
}
