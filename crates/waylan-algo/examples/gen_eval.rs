//! 大规模离线评测集自动生成工具
//!
//! 从 Rime 格式词库（如 `data/dicts/2_base/rime_mint.base.dict.yaml`）中抽样生成覆盖五大场景的
//! 标准 3 列 TSV 评测集（`category\tinput\texpected`），供 `eval_bench` 执行大样本回归与训练：
//! 1. `homophone`：高频同音词及中低频精确词保护用例；
//! 2. `partial`：末尾截断 1~2 字母的半成品补全用例；
//! 3. `jianpin`：纯首字母简拼（3~4 字词）与全拼混简拼（首音节全拼+其余首字母）用例；
//! 4. `typo`：邻键替换、相邻倒置、漏按、多按四类常见误输用例；
//! 5. `sentence`：多词拼接长句用例。

use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use waylan_algo::syllables::get_qwerty_neighbors;

/// 词典抽样条目（词文本、无声调音节列表、原始按键串、词频权重）
#[derive(Debug, Clone)]
struct DictItem {
    text: String,
    syllables: Vec<String>,
    joined: String,
    weight: u32,
}

fn arg_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|pos| args.get(pos + 1))
        .map(String::as_str)
}

/// 将带声调拼音音节规范化为纯 ASCII 小写拼音（`ü` -> `v`）
fn strip_pinyin_tones(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let mapped = match ch {
            'ā' | 'á' | 'ǎ' | 'à' | 'Ā' | 'Á' | 'Ǎ' | 'À' => 'a',
            'ē' | 'é' | 'ě' | 'è' | 'Ē' | 'É' | 'Ě' | 'È' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' | 'Ī' | 'Í' | 'Ǐ' | 'Ì' => 'i',
            'ō' | 'ó' | 'ǒ' | 'ò' | 'Ō' | 'Ó' | 'Ǒ' | 'Ò' => 'o',
            'ū' | 'ú' | 'ǔ' | 'ù' | 'Ū' | 'Ú' | 'Ǔ' | 'Ù' => 'u',
            'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' | 'ü' | 'Ǖ' | 'Ǘ' | 'Ǚ' | 'Ǜ' | 'Ü' => 'v',
            other => other.to_ascii_lowercase(),
        };
        if mapped.is_ascii_lowercase() {
            out.push(mapped);
        }
    }
    out
}

/// 解析 Rime YAML 词库文件并按词频降序返回合法 2~4 字词条目
fn load_dict_items(path: &Path) -> Result<Vec<DictItem>, String> {
    let file =
        File::open(path).map_err(|e| format!("打开词库文件失败 ({}): {e}", path.display()))?;
    let reader = BufReader::new(file);
    let mut items = Vec::new();
    let mut in_body = false;

    for line_res in reader.lines() {
        let Ok(line) = line_res else {
            continue;
        };
        let trimmed = line.trim();
        if !in_body {
            if trimmed == "..." {
                in_body = true;
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split('\t');
        let (Some(text), Some(py_raw), Some(weight_raw)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let char_count = text.chars().count();
        if !(2..=4).contains(&char_count) {
            continue;
        }
        let Ok(weight) = weight_raw.trim().parse::<u32>() else {
            continue;
        };
        if weight < 50 {
            continue;
        }
        let syllables: Vec<String> = py_raw
            .split_whitespace()
            .map(strip_pinyin_tones)
            .filter(|s| !s.is_empty())
            .collect();
        if syllables.len() != char_count {
            continue;
        }
        let joined = syllables.concat();
        if !(4..=20).contains(&joined.len()) {
            continue;
        }
        items.push(DictItem {
            text: text.to_string(),
            syllables,
            joined,
            weight,
        });
    }

    items.sort_by_key(|item| std::cmp::Reverse(item.weight));
    Ok(items)
}

/// 生成同音词（高频 + 中低频精确保护）用例
fn gen_homophone_cases(items: &[DictItem], limit: usize) -> Vec<(String, String)> {
    let mut by_py: BTreeMap<&str, Vec<&DictItem>> = BTreeMap::new();
    for item in items {
        by_py.entry(item.joined.as_str()).or_default().push(item);
    }

    let mut out = Vec::with_capacity(limit);
    let mut seen = HashSet::new();
    let half = limit / 2;

    // 1. 高频同音首位词
    for item in items.iter().take(5_000) {
        if out.len() >= half {
            break;
        }
        if seen.insert(item.joined.as_str())
            && let Some(group) = by_py.get(item.joined.as_str())
        {
            out.push((item.joined.clone(), group[0].text.clone()));
        }
    }

    // 2. 中低频精确词（跳过前 8_000 高频词，专门检验不被高频前缀半成品抢占）
    for item in items.iter().skip(8_000) {
        if out.len() >= limit {
            break;
        }
        if let Some(group) = by_py.get(item.joined.as_str())
            && group[0].text == item.text
            && seen.insert(item.joined.as_str())
        {
            out.push((item.joined.clone(), item.text.clone()));
        }
    }
    out
}

/// 生成末尾截断 1~2 字母的半成品补全用例
fn gen_partial_cases(items: &[DictItem], limit: usize) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(limit);
    let mut seen = HashSet::new();
    for (idx, item) in items.iter().take(4_000).enumerate() {
        if out.len() >= limit {
            break;
        }
        let cut = if idx % 2 == 0 { 1 } else { 2 };
        if item.joined.len() <= cut + 3 {
            continue;
        }
        let truncated = &item.joined[..item.joined.len() - cut];
        if seen.insert(truncated.to_string()) {
            out.push((truncated.to_string(), item.text.clone()));
        }
    }
    out
}

/// 生成纯简拼（3~4 字词）与全拼混简拼用例
fn gen_jianpin_cases(items: &[DictItem], limit: usize) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(limit);
    let mut seen = HashSet::new();
    let half = limit / 2;

    // 1. 纯首字母简拼（仅取 3~4 字高频词）
    for item in items.iter().take(8_000) {
        if out.len() >= half {
            break;
        }
        if item.syllables.len() < 3 {
            continue;
        }
        let acronym: String = item
            .syllables
            .iter()
            .filter_map(|s| s.chars().next())
            .collect();
        if acronym.len() == item.syllables.len() && seen.insert(acronym.clone()) {
            out.push((acronym, item.text.clone()));
        }
    }

    // 2. 全拼混简拼（首音节完整全拼 + 其余音节声母）
    for item in items.iter().take(8_000) {
        if out.len() >= limit {
            break;
        }
        if item.syllables.len() < 3 {
            continue;
        }
        let mut mixed = item.syllables[0].clone();
        for syl in &item.syllables[1..] {
            if let Some(ch) = syl.chars().next() {
                mixed.push(ch);
            }
        }
        if seen.insert(mixed.clone()) {
            out.push((mixed, item.text.clone()));
        }
    }
    out
}

/// 生成四类常见误输纠错（邻键、倒置、漏按、多按）用例
fn gen_typo_cases(items: &[DictItem], limit: usize) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(limit);
    let mut seen = HashSet::new();
    let exact_set: HashSet<&str> = items.iter().map(|i| i.joined.as_str()).collect();

    for (idx, item) in items.iter().take(5_000).enumerate() {
        if out.len() >= limit {
            break;
        }
        let bytes = item.joined.as_bytes();
        if bytes.len() < 5 {
            continue;
        }
        let mutated = match idx % 4 {
            // 邻键替换（替换中间某字母为 QWERTY 最近邻键）
            0 => {
                let pos = bytes.len() / 2;
                let orig = bytes[pos] as char;
                let Some(&(alt, _)) = get_qwerty_neighbors(orig).first() else {
                    continue;
                };
                let mut buf = bytes.to_vec();
                buf[pos] = alt as u8;
                String::from_utf8(buf).ok()
            }
            // 相邻字母倒置
            1 => {
                let pos = bytes.len() / 2;
                let mut buf = bytes.to_vec();
                if buf[pos - 1] == buf[pos] {
                    continue;
                }
                buf.swap(pos - 1, pos);
                String::from_utf8(buf).ok()
            }
            // 漏按 1 字母（优先删除高频漏按字母 g/n/a/e/i/o/u）
            2 => {
                let pos = (1..bytes.len() - 1).find(|&i| b"gnaeiou".contains(&bytes[i]));
                pos.and_then(|p| {
                    let mut buf = Vec::with_capacity(bytes.len() - 1);
                    buf.extend_from_slice(&bytes[..p]);
                    buf.extend_from_slice(&bytes[p + 1..]);
                    String::from_utf8(buf).ok()
                })
            }
            // 多按 1 字母（末尾重复连击）
            _ => Some(format!("{}{}", item.joined, bytes[bytes.len() - 1] as char)),
        };

        if let Some(m) = mutated
            && !exact_set.contains(m.as_str())
            && seen.insert(m.clone())
        {
            out.push((m, item.text.clone()));
        }
    }
    out
}

/// 生成双词组合长句用例
fn gen_sentence_cases(items: &[DictItem], limit: usize) -> Vec<(String, String)> {
    let pool: Vec<&DictItem> = items
        .iter()
        .filter(|i| i.syllables.len() == 2 && i.weight >= 5_000)
        .take(300)
        .collect();
    let mut out = Vec::with_capacity(limit);
    let mut seen = HashSet::new();

    if pool.len() < 4 {
        return out;
    }
    for i in 0..pool.len() {
        for step in [7usize, 13, 29] {
            if out.len() >= limit {
                return out;
            }
            let j = (i + step) % pool.len();
            if i == j {
                continue;
            }
            let py = format!("{}{}", pool[i].joined, pool[j].joined);
            let txt = format!("{}{}", pool[i].text, pool[j].text);
            if seen.insert(py.clone()) {
                out.push((py, txt));
            }
        }
    }
    out
}

fn write_tsv(out_path: &Path, groups: &[(&str, Vec<(String, String)>)]) -> Result<usize, String> {
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| format!("创建输出目录失败: {e}"))?;
    }
    let file = File::create(out_path).map_err(|e| format!("创建输出文件失败: {e}"))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "# 自动生成的 WayLan 五大场景离线评测集")
        .map_err(|e| format!("写入失败: {e}"))?;
    writeln!(writer, "# 格式: category<TAB>input<TAB>expected")
        .map_err(|e| format!("写入失败: {e}"))?;

    let mut total = 0usize;
    for (cat, rows) in groups {
        for (input, expected) in rows {
            writeln!(writer, "{cat}\t{input}\t{expected}").map_err(|e| format!("写入失败: {e}"))?;
            total += 1;
        }
    }
    writer
        .flush()
        .map_err(|e| format!("刷新输出缓冲失败: {e}"))?;
    Ok(total)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let dict_path =
        arg_value(&args, "--dict").unwrap_or("data/dicts/2_base/rime_mint.base.dict.yaml");
    let out_path = arg_value(&args, "--out").unwrap_or("target/eval_1000.tsv");
    let per_cat: usize = arg_value(&args, "--per-category")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    let items = match load_dict_items(Path::new(dict_path)) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };
    println!("已解析词库 {} 条有效 2~4 字词", items.len());

    let groups = [
        ("homophone", gen_homophone_cases(&items, per_cat)),
        ("partial", gen_partial_cases(&items, per_cat)),
        ("jianpin", gen_jianpin_cases(&items, per_cat)),
        ("typo", gen_typo_cases(&items, per_cat)),
        ("sentence", gen_sentence_cases(&items, per_cat)),
    ];
    for (name, rows) in &groups {
        println!("  生成分类 {:<10}: {} 条", name, rows.len());
    }

    match write_tsv(Path::new(out_path), &groups) {
        Ok(total) => println!("已输出共计 {total} 条评测用例至 {out_path}"),
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    }
}
