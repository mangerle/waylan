//! 纯文本空格/制表符词库流式解析器

use std::io::{BufRead, Cursor};
use std::path::Path;

use waylan_algo::types::ImportedDictEntry;

use super::DictImporter;
use crate::error::DictImportError;

/// 流式逐行解析纯文本词库（复用行缓冲，避免大文件全量读入内存）
pub(super) fn parse_plain_text_reader<R: BufRead>(
    mut reader: R,
    path: &Path,
) -> Result<Vec<ImportedDictEntry>, DictImportError> {
    let mut entries = Vec::new();
    let mut line_buf = String::new();

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
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let entry = if line.contains('\t') {
            DictImporter::parse_entry_columns(line.split('\t'))
        } else {
            DictImporter::parse_entry_columns(line.split_whitespace())
        };
        if let Some(entry) = entry {
            entries.push(entry);
        }
    }

    Ok(entries)
}

/// 解析内存中的纯文本词库
pub(super) fn parse_plain_text(content: &str) -> Result<Vec<ImportedDictEntry>, DictImportError> {
    parse_plain_text_reader(Cursor::new(content.as_bytes()), Path::new("<memory>"))
}
