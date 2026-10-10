use std::path::Path;
use waylan_algo::builder::DictBuilder;
use waylan_algo::opencc::OpenCcDict;
use waylan_algo::AlgoEngine;

/// 处理 `dict` 命令行子命令
pub fn handle_dict_subcommand(sub_args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if sub_args.is_empty() {
        print_dict_usage();
        return Ok(());
    }

    match sub_args[0].as_str() {
        "--help" | "-h" => {
            print_dict_usage();
        }
        "build" => {
            if sub_args.len() < 3 {
                print_dict_usage();
                return Ok(());
            }
            let output_path = &sub_args[sub_args.len() - 1];
            let input_paths = &sub_args[1..sub_args.len() - 1];
            log::info!(
                "[词库构建] 正在将文本词库编译为零拷贝 DAT 紧凑二进制词库: {}",
                output_path
            );
            let stats =
                DictBuilder::build_compact_binary_dict(input_paths, Path::new(output_path))?;
            log::info!(
                "[构建完成] 行数: {}, 有效词条: {}, 键数: {}, 耗时: {} ms",
                stats.total_lines,
                stats.total_words,
                stats.total_keys,
                stats.elapsed_ms
            );
        }
        "build-opencc" => {
            let (chars_path, phrases_path, output_path) = if sub_args.len() >= 4 {
                (&sub_args[1], &sub_args[2], &sub_args[3])
            } else {
                (
                    &"/usr/share/opencc/STCharacters.txt".to_string(),
                    &"/usr/share/opencc/STPhrases.txt".to_string(),
                    &"opencc.dict.bin".to_string(),
                )
            };
            log::info!(
                "[简繁词库] 正在将 OpenCC 词表编译为二进制紧凑词库: {} + {} -> {}",
                chars_path,
                phrases_path,
                output_path
            );
            let dict = OpenCcDict::from_tsv_files(chars_path, phrases_path)?;
            dict.save_to_file(output_path)?;
            log::info!(
                "[简繁词库] 编译完成，短语数: {}, 单字数: {}",
                dict.phrases_len(),
                dict.chars.len()
            );
        }
        "build-aux" => {
            if sub_args.len() < 4 {
                print_dict_usage();
                return Ok(());
            }
            let english_path = &sub_args[1];
            let symbols_path = &sub_args[2];
            let output_path = &sub_args[3];
            log::info!(
                "[辅助词库] 编译英文与符号表情词库: {} + {} -> {}",
                english_path,
                symbols_path,
                output_path
            );
            let stats =
                DictBuilder::build_auxiliary_dict(&[english_path], &[symbols_path], output_path)?;
            log::info!(
                "[辅助词库] 编译完成，英文词汇数: {}, 符号表情数: {}",
                stats.english_words.len(),
                stats.symbols.len()
            );
        }
        "import-user" => {
            if sub_args.len() < 3 {
                print_dict_usage();
                return Ok(());
            }
            let input_path = &sub_args[1];
            let user_dict_path = &sub_args[2];
            log::info!(
                "[用户词库] 从文本导入自造词与调频记录: {} -> {}",
                input_path,
                user_dict_path
            );
            let mut engine = AlgoEngine::default();
            engine.set_user_dict_path(user_dict_path);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            let count =
                DictBuilder::import_user_text_dict(input_path, &mut engine.user_model, now)?;
            engine.persist_user_dict()?;
            log::info!("[用户词库] 成功导入 {} 条用户记录", count);
        }
        other => {
            log::warn!("未知的词库管理子命令: {}", other);
            print_dict_usage();
        }
    }

    Ok(())
}

/// 打印词库管理 CLI 使用帮助
pub fn print_dict_usage() {
    log::info!("微澜输入法 (WayLan) 词库构建管理工具");
    log::info!("用法: waylan dict <子命令> [参数...]");
    log::info!("  build <输入文本词库1> [输入2...] <输出二进制文件>");
    log::info!("  build-opencc [STCharacters.txt] [STPhrases.txt] [输出文件]");
    log::info!("  build-aux <英文词表> <符号表> <输出文件>");
    log::info!("  import-user <用户文本词库> <用户二进制WAL路径>");
}

/// 执行算法与词典自检测试
pub fn run_self_check(engine: &mut AlgoEngine) -> Result<(), Box<dyn std::error::Error>> {
    log::info!("=== 微澜核心算法引擎自检 ===");
    for ch in "xian".chars() {
        engine.push_char(ch);
    }
    let cands = engine.candidates();
    log::info!("输入: 'xian' => 召回候选数: {}", cands.len());
    for (i, c) in cands.iter().enumerate().take(5) {
        log::info!(
            "  [{}] {} (拼音: {}, 得分: {})",
            i + 1,
            c.text,
            c.pinyin,
            c.score
        );
    }
    engine.reset();
    log::info!("自检测试通过！");
    Ok(())
}
