//! 外部词库与字典数据导入模块
//!
//! # 设计原理
//! - **实现初衷**：为用户提供外部拼音词库、英文词库及特殊符号表的增量导入与合入机制。
//! - **核心优势**：
//!   1. 统一安全防御机制，校验 64MB 文件体积上限，杜绝内存爆满风险；
//!   2. 支持自动去重与增量合入，避免污染现有自适应词库与预设字典；
//!   3. 支持 WAL 日志紧凑化写入，确保断电或异常退出时数据一致性。
//! - **代价与局限**：
//!   一次性同步解析文本行，超大文件导入时存在短暂的 I/O 处理耗时。

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use waylan_algo::dict_importer::DictImporter;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_algo::AuxiliaryDict;
use waylan_core::config::AppConfig;

/// 外部导入文件体积最大允许上限（64 MB），防止异常大文件导致内存暴涨
pub const MAX_IMPORT_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 解析并合入外部拼音词库
///
/// # Errors
/// 当文件不存在、体积超限、解析格式错误或写入磁盘失败时返回错误描述。
pub fn import_pinyin_dict_file(path_str: &str) -> Result<String, String> {
    let path = Path::new(path_str.trim());
    if !path.is_file() {
        return Err("指定的词库文件不存在或路径无效，请核对后重试。".to_string());
    }
    if path
        .metadata()
        .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
        .unwrap_or(true)
    {
        return Err("词库文件体积超过 64MB 安全上限，已拒绝加载。".to_string());
    }

    let entries = DictImporter::import_file(path).map_err(|e| format!("解析词库失败: {}", e))?;
    let count = entries.len();
    let user_dict_path = AppConfig::user_dict_path();
    let mut user_model = UserAdaptiveModel::load_or_replay_wal(&user_dict_path).unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_700_000_000);
    for entry in &entries {
        user_model.record_commit(&entry.text, &entry.pinyin, now);
    }
    user_model
        .compact_wal(&user_dict_path)
        .map_err(|e| format!("保存用户词库失败: {}", e))?;

    Ok(format!(
        "成功合入 {} 条词目，当前自适应词库总词量: {} 条",
        count,
        user_model.total_entries()
    ))
}

/// 解析并合入外部英文单词库
///
/// # Errors
/// 当文件不存在、体积超限、读取失败或写入磁盘失败时返回错误描述。
pub fn import_english_dict_file(
    path_str: &str,
    aux: &mut AuxiliaryDict,
) -> Result<(String, usize), String> {
    let path = Path::new(path_str.trim());
    if !path.is_file() {
        return Err("指定的英文词典文件不存在，请核对路径。".to_string());
    }
    if path
        .metadata()
        .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
        .unwrap_or(true)
    {
        return Err("英文词库体积超过 64MB 安全上限。".to_string());
    }

    let content = fs::read_to_string(path).map_err(|e| format!("读取文件失败: {}", e))?;
    let mut existing: HashSet<String> = aux
        .english_words
        .iter()
        .map(|(w, _)| w.to_lowercase())
        .collect();
    let mut added = 0;
    let mut in_yaml_header = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            in_yaml_header = true;
            continue;
        }
        if trimmed == "..." {
            in_yaml_header = false;
            continue;
        }
        if in_yaml_header || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let word = trimmed.split('\t').next().unwrap_or("").trim();
        if !word.is_empty()
            && word.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
            && existing.insert(word.to_lowercase())
        {
            aux.english_words.push((word.to_string(), 50_000));
            added += 1;
        }
    }

    let aux_path = AppConfig::user_aux_dict_path();
    aux.save_to_file(&aux_path)
        .map_err(|e| format!("保存用户辅助词库失败: {}", e))?;

    Ok((
        format!(
            "成功导入 {} 个英文单词，当前单词库总数: {}",
            added,
            aux.english_words.len()
        ),
        aux.english_words.len(),
    ))
}

/// 解析并合入外部符号映射表
///
/// # Errors
/// 当文件不存在、体积超限、读取失败或写入磁盘失败时返回错误描述。
pub fn import_symbols_dict_file(
    path_str: &str,
    aux: &mut AuxiliaryDict,
) -> Result<(String, usize), String> {
    let path = Path::new(path_str.trim());
    if !path.is_file() {
        return Err("指定的符号映射文件不存在，请核对路径。".to_string());
    }
    if path
        .metadata()
        .map(|m| m.len() > MAX_IMPORT_FILE_BYTES)
        .unwrap_or(true)
    {
        return Err("符号词库体积超过 64MB 安全上限。".to_string());
    }

    let content = fs::read_to_string(path).map_err(|e| format!("读取文件失败: {}", e))?;
    let mut existing: HashSet<(String, String)> = aux
        .symbols
        .iter()
        .map(|(k, s, _)| (k.clone(), s.clone()))
        .collect();
    let mut added = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() >= 2 {
            let key = parts[0].trim().to_string();
            let sym = parts[1].trim().to_string();
            let desc = if parts.len() >= 3 {
                parts[2].trim().to_string()
            } else {
                "外部符号".to_string()
            };
            if existing.insert((key.clone(), sym.clone())) {
                aux.symbols.push((key, sym, desc));
                added += 1;
            }
        }
    }

    let aux_path = AppConfig::user_aux_dict_path();
    aux.save_to_file(&aux_path)
        .map_err(|e| format!("保存用户符号词库失败: {}", e))?;

    Ok((
        format!(
            "成功导入 {} 组符号联想项，当前符号总数: {}",
            added,
            aux.symbols.len()
        ),
        aux.symbols.len(),
    ))
}
