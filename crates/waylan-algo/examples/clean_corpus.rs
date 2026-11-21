//! 多源中文语料流式清洗与分句规范化工具
//!
//! # 设计原理
//! - **实现初衷**：将下载的维基百科、THUCNews 新闻及本地知识/对话语料统一清洗为
//!   纯净的连续简体中文子句，供后续与 `base.dict.bin` 词表对齐分词并统计 Bigram/Unigram。
//! - **核心优势**：零第三方依赖流式处理 GB 级语料，自动按标点与非汉字边界断句，
//!   杜绝跨标点统计虚假二元共现。

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

/// 单个语料清洗任务配置
struct CorpusTask {
    name: &'static str,
    input_path: &'static str,
    output_path: &'static str,
    is_jsonl_text: bool,
    is_tsv_body: bool,
}

/// 清洗统计结果
struct CleanStats {
    lines_read: usize,
    sentences_out: usize,
    hanzi_count: usize,
}

fn main() {
    let out_dir = Path::new("/home/wsl/coding/waylan/data/corpus_clean");
    if let Err(e) = fs::create_dir_all(out_dir) {
        eprintln!("创建输出目录失败: {e}");
        return;
    }

    let tasks = [
        CorpusTask {
            name: "维基百科 (zh-cn)",
            input_path: "/home/wsl/coding/waylan/data/corpus_raw/wikipedia-zh-cn.json",
            output_path: "/home/wsl/coding/waylan/data/corpus_clean/wiki_clean.txt",
            is_jsonl_text: true,
            is_tsv_body: false,
        },
        CorpusTask {
            name: "THUCNews 新闻",
            input_path: "/home/wsl/coding/waylan/data/corpus_raw/thucnews.txt",
            output_path: "/home/wsl/coding/waylan/data/corpus_clean/thucnews_clean.txt",
            is_jsonl_text: false,
            is_tsv_body: true,
        },
        CorpusTask {
            name: "MiniMind 知识语料",
            input_path:
                "/mnt/d/coding/rust/minimind/dataset/waylan_work/knowledge_minimind_clean.jsonl",
            output_path: "/home/wsl/coding/waylan/data/corpus_clean/knowledge_clean.txt",
            is_jsonl_text: true,
            is_tsv_body: false,
        },
        CorpusTask {
            name: "LCCC 口语对话",
            input_path: "/mnt/d/coding/rust/minimind/dataset/waylan_work/spoken_lccc_clean.jsonl",
            output_path: "/home/wsl/coding/waylan/data/corpus_clean/spoken_lccc_clean.txt",
            is_jsonl_text: true,
            is_tsv_body: false,
        },
    ];

    for task in &tasks {
        let t0 = Instant::now();
        match process_task(task) {
            Ok(stats) => {
                println!(
                    "[完成] {} | 原行数: {} | 规范句子数: {} | 总汉字数: {} | 耗时: {:.2}s",
                    task.name,
                    stats.lines_read,
                    stats.sentences_out,
                    stats.hanzi_count,
                    t0.elapsed().as_secs_f64()
                );
            }
            Err(e) => eprintln!("[失败] {}: {e}", task.name),
        }
    }
}

/// 流式处理单个语料文件并写入清洗后的分句结果
fn process_task(task: &CorpusTask) -> Result<CleanStats, String> {
    let in_file = File::open(task.input_path)
        .map_err(|e| format!("打开输入文件 {} 失败: {e}", task.input_path))?;
    let out_file = File::create(task.output_path)
        .map_err(|e| format!("创建输出文件 {} 失败: {e}", task.output_path))?;

    let mut reader = BufReader::with_capacity(1024 * 1024, in_file);
    let mut writer = BufWriter::with_capacity(1024 * 1024, out_file);

    let mut raw_line = String::with_capacity(4096);
    let mut extracted = String::with_capacity(4096);
    let mut sentence_buf = String::with_capacity(256);

    let mut stats = CleanStats {
        lines_read: 0,
        sentences_out: 0,
        hanzi_count: 0,
    };

    loop {
        raw_line.clear();
        let bytes = reader
            .read_line(&mut raw_line)
            .map_err(|e| format!("读取行失败: {e}"))?;
        if bytes == 0 {
            break;
        }
        stats.lines_read += 1;

        let text = if task.is_jsonl_text {
            extracted.clear();
            if extract_json_text_field(&raw_line, &mut extracted) {
                extracted.as_str()
            } else {
                continue;
            }
        } else if task.is_tsv_body {
            raw_line
                .split_once('\t')
                .map(|(_, body)| body)
                .unwrap_or(raw_line.as_str())
        } else {
            raw_line.as_str()
        };

        emit_clean_sentences(text, &mut sentence_buf, &mut writer, &mut stats)?;
    }

    writer.flush().map_err(|e| format!("刷新输出失败: {e}"))?;
    Ok(stats)
}

/// 从 JSONL 行中轻量提取 `"text"` 字段并处理常见转义
fn extract_json_text_field(line: &str, out: &mut String) -> bool {
    let key = "\"text\"";
    let Some(pos) = line.find(key) else {
        return false;
    };
    let rest = &line[pos + key.len()..];
    let Some(colon) = rest.find(':') else {
        return false;
    };
    let after_colon = rest[colon + 1..].trim_start();
    let Some(stripped) = after_colon.strip_prefix('"') else {
        return false;
    };

    let mut chars = stripped.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return true,
            '\\' => match chars.next() {
                Some('n' | 'r' | 't') => out.push('\n'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some(other) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    !out.is_empty()
}

/// 判断字符是否为常用基本简体/通用汉字（CJK 统一表意文字基本区）
#[inline]
fn is_cjk_basic(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch)
}

/// 剥离 URL 与 HTML 标签后，按非基本汉字边界切分并写出长度在 2..=64 之间的纯净中文子句
fn emit_clean_sentences(
    text: &str,
    buf: &mut String,
    writer: &mut BufWriter<File>,
    stats: &mut CleanStats,
) -> Result<(), String> {
    buf.clear();
    let mut char_count = 0usize;
    let mut in_tag = false;

    for ch in text.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
            }
            continue;
        }
        if ch == '<' {
            flush_sentence(buf, char_count, writer, stats)?;
            char_count = 0;
            in_tag = true;
            continue;
        }
        if is_cjk_basic(ch) {
            buf.push(ch);
            char_count += 1;
        } else {
            flush_sentence(buf, char_count, writer, stats)?;
            char_count = 0;
        }
    }
    flush_sentence(buf, char_count, writer, stats)
}

/// 将满足长度约束（2..=64 汉字）的句子写入输出文件
fn flush_sentence(
    buf: &mut String,
    char_count: usize,
    writer: &mut BufWriter<File>,
    stats: &mut CleanStats,
) -> Result<(), String> {
    if (2..=64).contains(&char_count) {
        writer
            .write_all(buf.as_bytes())
            .and_then(|_| writer.write_all(b"\n"))
            .map_err(|e| format!("写入输出失败: {e}"))?;
        stats.sentences_out += 1;
        stats.hanzi_count += char_count;
    }
    buf.clear();
    Ok(())
}
