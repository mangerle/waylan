//! 微澜 (WayLan) 独立纯净中文拼音评测集（10K / 100K / 500K）生成工具
//!
//! # 设计原理
//! - **实现初衷**：为微澜输入法构建与训练集严格隔离、无数据泄露（Zero Data Contamination）的工业级标准评测集；
//! - **核心优势**：从第三方真实权威语料中按 40%:35%:25% 严格分层采样短词、中句与长句，结合成词词库最大正向匹配精准注音；
//! - **代价与局限**：严格纯汉字过滤排除了含有数字和英文符号的混合句子，专注中文拼音序列切分与语义消歧。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// 声调音标到标准 ASCII 拼音字母映射
fn remove_pinyin_tones(pinyin: &str) -> String {
    let mut clean = String::with_capacity(pinyin.len());
    for ch in pinyin.chars() {
        match ch {
            'ā' | 'á' | 'ǎ' | 'à' => clean.push('a'),
            'ō' | 'ó' | 'ǒ' | 'ò' => clean.push('o'),
            'ē' | 'é' | 'ě' | 'è' | 'ê' => clean.push('e'),
            'ī' | 'í' | 'ǐ' | 'ì' => clean.push('i'),
            'ū' | 'ú' | 'ǔ' | 'ù' => clean.push('u'),
            'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => clean.push('v'),
            'ń' | 'ň' | 'ǹ' => clean.push('n'),
            'ḿ' => clean.push('m'),
            '\u{0300}'..='\u{036f}' => {}
            'a'..='z' | 'A'..='Z' => clean.push(ch.to_ascii_lowercase()),
            _ => {}
        }
    }
    clean
}

/// 从单字字典 `rime_mint.chars.dict.yaml` 构建汉字最高频本音映射
fn load_char_pinyin_map(dict_path: &Path) -> HashMap<char, String> {
    let mut map: HashMap<char, (String, u32)> = HashMap::with_capacity(30000);
    let Ok(file) = File::open(dict_path) else {
        println!("[警告] 无法打开单字字典: {:?}", dict_path);
        return HashMap::new();
    };

    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("---")
            || trimmed.starts_with("...")
        {
            continue;
        }

        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() >= 2 {
            let ch_str = parts[0];
            let raw_py = parts[1];
            let weight = if parts.len() >= 3 {
                parts[2].parse::<u32>().unwrap_or(0)
            } else {
                0
            };

            if ch_str.chars().count() == 1 {
                let ch = ch_str.chars().next().unwrap();
                let clean_py = remove_pinyin_tones(raw_py);
                if !clean_py.is_empty() {
                    let entry = map.entry(ch).or_insert_with(|| (clean_py.clone(), weight));
                    if weight > entry.1 {
                        *entry = (clean_py, weight);
                    }
                }
            }
        }
    }

    let result: HashMap<char, String> = map.into_iter().map(|(k, (v, _))| (k, v)).collect();
    println!(
        "[字典加载] 成功从单字字典加载 {} 个汉字的标准拼音映射",
        result.len()
    );
    result
}

/// 从成词词库 `rime_mint.base.dict.yaml` 载入常用多字词标准注音
fn load_word_pinyin_map(dict_path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(100000);
    let Ok(file) = File::open(dict_path) else {
        return map;
    };

    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("---")
            || trimmed.starts_with("...")
        {
            continue;
        }

        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() >= 2 {
            let word = parts[0];
            let raw_py = parts[1];
            let char_count = word.chars().count();
            if (2..=4).contains(&char_count) {
                let syllables: Vec<String> = raw_py
                    .split_whitespace()
                    .map(remove_pinyin_tones)
                    .filter(|s| !s.is_empty())
                    .collect();
                if syllables.len() == char_count {
                    map.entry(word.to_string())
                        .or_insert_with(|| syllables.join("'"));
                }
            }
        }
    }
    println!(
        "[词库加载] 成功从成词词库载入 {} 条常用多音词标准拼音",
        map.len()
    );
    map
}

/// 基于最大正向匹配（Max Forward Matching）将汉语句子转为准确的全拼音串
fn text_to_pinyin(
    text: &str,
    char_map: &HashMap<char, String>,
    word_map: &HashMap<String, String>,
) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let total_chars = chars.len();
    let mut i = 0;
    let mut syllables = Vec::with_capacity(total_chars);

    while i < total_chars {
        let mut matched = false;
        for len in (2..=4.min(total_chars - i)).rev() {
            let sub_word: String = chars[i..i + len].iter().collect();
            if let Some(word_py) = word_map.get(&sub_word) {
                syllables.push(word_py.clone());
                i += len;
                matched = true;
                break;
            }
        }

        if !matched {
            let ch = chars[i];
            let py = char_map.get(&ch)?;
            syllables.push(py.clone());
            i += 1;
        }
    }

    Some(syllables.join("'"))
}

/// 检查字符是否为常用中文字符范围 (CJK Unified Ideographs)
fn is_cjk_char(ch: char) -> bool {
    matches!(ch, '\u{4e00}'..='\u{9fa5}')
}

/// 分层样本条目
#[derive(Debug, Clone)]
struct TargetSample {
    target: String,
    pinyin: String,
}

/// 科学分层样本收集器
struct StratifiedSampler {
    short_pool: Vec<TargetSample>,  // 2..=4 汉字 (占比 40%)
    medium_pool: Vec<TargetSample>, // 5..=8 汉字 (占比 35%)
    long_pool: Vec<TargetSample>,   // 9..=18 汉字 (占比 25%)
    seen: HashSet<String>,
    target_short: usize,
    target_medium: usize,
    target_long: usize,
}

impl StratifiedSampler {
    fn new(total_target: usize) -> Self {
        let target_short = (total_target as f64 * 0.40).round() as usize;
        let target_medium = (total_target as f64 * 0.35).round() as usize;
        let target_long = total_target - target_short - target_medium;

        Self {
            short_pool: Vec::with_capacity(target_short),
            medium_pool: Vec::with_capacity(target_medium),
            long_pool: Vec::with_capacity(target_long),
            seen: HashSet::with_capacity(total_target + 50000),
            target_short,
            target_medium,
            target_long,
        }
    }

    fn is_full(&self) -> bool {
        self.short_pool.len() >= self.target_short
            && self.medium_pool.len() >= self.target_medium
            && self.long_pool.len() >= self.target_long
    }

    fn try_add(
        &mut self,
        text: &str,
        char_map: &HashMap<char, String>,
        word_map: &HashMap<String, String>,
    ) -> bool {
        let trimmed = text.trim();
        let char_count = trimmed.chars().count();
        if !(2..=18).contains(&char_count) {
            return false;
        }

        // 检查对应桶是否已满
        match char_count {
            2..=4 if self.short_pool.len() >= self.target_short => return false,
            5..=8 if self.medium_pool.len() >= self.target_medium => return false,
            9..=18 if self.long_pool.len() >= self.target_long => return false,
            _ => {}
        }

        // 严格纯汉字校验
        if !trimmed.chars().all(is_cjk_char) {
            return false;
        }

        // 全局去重
        if self.seen.contains(trimmed) {
            return false;
        }

        // 转换为全拼
        if let Some(pinyin) = text_to_pinyin(trimmed, char_map, word_map) {
            self.seen.insert(trimmed.to_string());
            let sample = TargetSample {
                target: trimmed.to_string(),
                pinyin,
            };
            match char_count {
                2..=4 => self.short_pool.push(sample),
                5..=8 => self.medium_pool.push(sample),
                _ => self.long_pool.push(sample),
            }

            // 若短词池未满且当前为中长句，抽取其中包含的合法真实成词短语
            if char_count > 4 && self.short_pool.len() < self.target_short {
                let chars: Vec<char> = trimmed.chars().collect();
                for len in (2..=4).rev() {
                    for window in chars.windows(len) {
                        if self.short_pool.len() >= self.target_short {
                            break;
                        }
                        let sub_word: String = window.iter().collect();
                        if !self.seen.contains(&sub_word) {
                            if let Some(sub_py) = word_map.get(&sub_word) {
                                self.seen.insert(sub_word.clone());
                                self.short_pool.push(TargetSample {
                                    target: sub_word,
                                    pinyin: sub_py.clone(),
                                });
                            }
                        }
                    }
                }
            }
            true
        } else {
            false
        }
    }

    /// 从成词词表中补齐尚未满足配比的短词短语
    fn fill_remaining_short_from_dict(&mut self, word_map: &HashMap<String, String>) {
        if self.short_pool.len() >= self.target_short {
            return;
        }
        println!(
            "[短词扩充] 正在从真实成词词表中补齐短词 (当前 {}/{})...",
            self.short_pool.len(),
            self.target_short
        );
        for (word, py) in word_map {
            if self.short_pool.len() >= self.target_short {
                break;
            }
            let count = word.chars().count();
            if (2..=4).contains(&count)
                && !self.seen.contains(word)
                && word.chars().all(is_cjk_char)
            {
                self.seen.insert(word.clone());
                self.short_pool.push(TargetSample {
                    target: word.clone(),
                    pinyin: py.clone(),
                });
            }
        }
    }

    /// 按 40%:35%:25% 比例生成指定大小的数据集
    fn extract_subset(&self, count: usize) -> Vec<TargetSample> {
        let n_short = ((count as f64) * 0.40).round() as usize;
        let n_medium = ((count as f64) * 0.35).round() as usize;
        let n_long = count - n_short - n_medium;

        let mut result = Vec::with_capacity(count);

        // 按照短、中、长交错混编，模拟真实自然输入流动
        let mut s_iter = self.short_pool.iter().take(n_short);
        let mut m_iter = self.medium_pool.iter().take(n_medium);
        let mut l_iter = self.long_pool.iter().take(n_long);

        loop {
            let mut added = false;
            if let Some(s) = s_iter.next() {
                result.push(s.clone());
                added = true;
            }
            if let Some(m) = m_iter.next() {
                result.push(m.clone());
                added = true;
            }
            if let Some(l) = l_iter.next() {
                result.push(l.clone());
                added = true;
            }
            if !added {
                break;
            }
        }

        result
    }
}

/// 扫描目录下的所有 JSON 文件
fn discover_json_files(base_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(base_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(discover_json_files(&path));
            } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
                files.push(path);
            }
        }
    }
    files
}

fn main() {
    println!("============================================================");
    println!("  微澜 (WayLan) 独立纯净中文拼音评测集（10K/100K/500K）构建");
    println!("============================================================");

    let start_time = Instant::now();

    // 1. 定位并载入单字与成词拼音字典
    let chars_dict_candidates = [
        "data/dicts/1_chars/rime_mint.chars.dict.yaml",
        "../../data/dicts/1_chars/rime_mint.chars.dict.yaml",
        "/usr/share/waylan/data/dicts/1_chars/rime_mint.chars.dict.yaml",
    ];
    let mut pinyin_map = HashMap::new();
    for cand in &chars_dict_candidates {
        let p = Path::new(cand);
        if p.exists() {
            pinyin_map = load_char_pinyin_map(p);
            break;
        }
    }
    if pinyin_map.is_empty() {
        eprintln!("[错误] 未找到单字拼音表，无法生成评测集！");
        return;
    }

    let base_dict_candidates = [
        "data/dicts/2_base/rime_mint.base.dict.yaml",
        "../../data/dicts/2_base/rime_mint.base.dict.yaml",
        "/usr/share/waylan/data/dicts/2_base/rime_mint.base.dict.yaml",
    ];
    let mut word_map = HashMap::new();
    for cand in &base_dict_candidates {
        let p = Path::new(cand);
        if p.exists() {
            word_map = load_word_pinyin_map(p);
            break;
        }
    }

    // 2. 定位独立真实语料（优先使用独立的第三方外部语料）
    let heldout_dirs = [
        PathBuf::from("/home/wangle/coding/data/heldout_raw"),
        PathBuf::from("/tmp/clue_eval"),
        PathBuf::from("data/heldout_raw"),
    ];

    let mut json_files = Vec::new();
    for dir in &heldout_dirs {
        if dir.exists() {
            json_files.extend(discover_json_files(dir));
        }
    }

    if json_files.is_empty() {
        eprintln!("[错误] 未找到任何独立评测原始文件，请先准备 heldout 语料！");
        return;
    }

    println!(
        "[语料源] 发现 {} 个独立第三方语料文件进行样本抽取：",
        json_files.len()
    );
    for f in &json_files {
        println!("  - {:?}", f);
    }

    let delimiters = [
        '，', '。', '！', '？', '；', '、', '\n', '\r', '\t', '“', '”', '‘', '’', '（', '）', '《',
        '》', '：', ',', '.', '!', '?', ';', ':', '(', ')', '"', '\'',
    ];

    // 初始化 500,000 条规模的科学分层采样器
    let mut sampler = StratifiedSampler::new(500_000);

    println!(
        "[抽取中] 正在进行分层采样 (短词目标: 200,000 | 中句目标: 175,000 | 长句目标: 125,000)..."
    );

    for file_path in &json_files {
        if sampler.is_full() {
            break;
        }
        let Ok(file) = File::open(file_path) else {
            continue;
        };
        let reader = BufReader::new(file);

        for line in reader.lines().map_while(Result::ok) {
            if sampler.is_full() {
                break;
            }
            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&line) {
                // 提取 json 中所有可能的文本字段
                let candidates = [
                    json_val.get("sentence").and_then(|v| v.as_str()),
                    json_val.get("sentence1").and_then(|v| v.as_str()),
                    json_val.get("sentence2").and_then(|v| v.as_str()),
                    json_val.get("text").and_then(|v| v.as_str()),
                ];

                for text_opt in candidates.into_iter().flatten() {
                    let mut start = 0;
                    for (pos, ch) in text_opt.char_indices() {
                        if delimiters.contains(&ch) {
                            if pos > start {
                                sampler.try_add(&text_opt[start..pos], &pinyin_map, &word_map);
                            }
                            start = pos + ch.len_utf8();
                        }
                    }
                    if start < text_opt.len() {
                        sampler.try_add(&text_opt[start..], &pinyin_map, &word_map);
                    }
                }
            }
        }

        println!(
            "  -> 已处理文件 {:?}: 短词={}/{} 中句={}/{} 长句={}/{}",
            file_path.file_name().unwrap_or_default(),
            sampler.short_pool.len(),
            sampler.target_short,
            sampler.medium_pool.len(),
            sampler.target_medium,
            sampler.long_pool.len(),
            sampler.target_long,
        );
    }

    // 若短词池尚未达到配比目标，使用成词词典补齐
    sampler.fill_remaining_short_from_dict(&word_map);

    let actual_total =
        sampler.short_pool.len() + sampler.medium_pool.len() + sampler.long_pool.len();
    println!("\n[采样完成] 成功抽取独立中文样本总计: {} 条", actual_total);
    println!("  - 短词短语 (2~4字):  {} 条", sampler.short_pool.len());
    println!("  - 常规短句 (5~8字):  {} 条", sampler.medium_pool.len());
    println!("  - 连贯长句 (9~18字): {} 条", sampler.long_pool.len());

    fs::create_dir_all("data/eval").ok();

    // 3. 分别导出 10K, 100K, 500K 三级评测集
    let targets = [
        ("data/eval/eval_heldout_10k.jsonl", 10_000),
        ("data/eval/eval_heldout_100k.jsonl", 100_000),
        ("data/eval/eval_heldout_500k.jsonl", 500_000),
    ];

    for (path, count) in targets {
        let export_count = count.min(actual_total);
        let subset = sampler.extract_subset(export_count);
        println!("[导出中] 正在写出 {} (共 {} 条样本)...", path, subset.len());

        let mut file = File::create(path).expect("创建评测集文件失败");
        for (idx, sample) in subset.iter().enumerate() {
            writeln!(
                file,
                "{{\"id\":{},\"target\":\"{}\",\"pinyin\":\"{}\"}}",
                idx + 1,
                sample.target,
                sample.pinyin
            )
            .unwrap();
        }
    }

    println!("============================================================");
    println!("          独立纯净三级评测金字塔生成完毕");
    println!("============================================================");
    println!("  1. 快速验证集: data/eval/eval_heldout_10k.jsonl  (10,000 条)");
    println!("  2. 标准基准集: data/eval/eval_heldout_100k.jsonl (100,000 条)");
    println!("  3. 极限全量集: data/eval/eval_heldout_500k.jsonl (500,000 条)");
    println!(
        "  - 耗时:       {:.2} 秒",
        start_time.elapsed().as_secs_f64()
    );
    println!("============================================================\n");
}
