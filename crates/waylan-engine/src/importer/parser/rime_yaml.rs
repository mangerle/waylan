//! Rime YAML 词典流式解析器

use std::io::{BufRead, Cursor};
use std::path::Path;

use waylan_algo::types::ImportedDictEntry;

use super::DictImporter;
use crate::error::DictImportError;

/// 流式逐行解析 Rime 词典 YAML（单遍扫描，复用行缓冲，避免大文件全量读入内存）
pub(super) fn parse_rime_yaml_reader<R: BufRead>(
    mut reader: R,
    path: &Path,
) -> Result<Vec<ImportedDictEntry>, DictImportError> {
    let mut entries = Vec::new();
    let mut line_buf = String::new();
    let mut seen_header_sep = false;

    loop {
        line_buf.clear();
        let n = reader
            .read_line(&mut line_buf)
            .map_err(|e| DictImportError::Io {
                path: path.to_path_buf(),
                source: e,
            })?;
        if n == 0 {
            break;
        }
        let line = line_buf.trim();
        if line.is_empty() || line.starts_with('#') || line == "---" {
            continue;
        }

        // Rime 词库表头以 "..." 分割正文；首次遇到 "..." 时丢弃此前可能误匹配的 YAML 元数据行
        if line == "..." {
            if !seen_header_sep {
                entries.clear();
                seen_header_sep = true;
            }
            continue;
        }

        if let Some(entry) = DictImporter::parse_entry_columns(line.split('\t')) {
            entries.push(entry);
        }
    }

    Ok(entries)
}

/// 解析内存中的 Rime YAML 词典文本
pub(super) fn parse_rime_yaml(content: &str) -> Result<Vec<ImportedDictEntry>, DictImportError> {
    parse_rime_yaml_reader(Cursor::new(content.as_bytes()), Path::new("<memory>"))
}
