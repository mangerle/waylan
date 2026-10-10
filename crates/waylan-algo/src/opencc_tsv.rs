use crate::error::AlgoError;
use crate::opencc::OpenCcDict;
use std::fs;
use std::path::{Path, PathBuf};

/// OpenCC TSV 格式词表导入解析器
pub struct OpenCcTsvParser;

impl OpenCcTsvParser {
    /// 从 OpenCC 标准 TSV 文本文件（`STCharacters.txt` 与 `STPhrases.txt`）解析构建词典
    pub fn from_tsv_files(
        chars_path: impl AsRef<Path>,
        phrases_path: impl AsRef<Path>,
    ) -> Result<OpenCcDict, AlgoError> {
        let chars_ref = chars_path.as_ref();
        let phrases_ref = phrases_path.as_ref();

        let chars_content = fs::read_to_string(chars_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(chars_ref),
            source: e,
        })?;
        let phrases_content =
            fs::read_to_string(phrases_ref).map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(phrases_ref),
                source: e,
            })?;

        let mut dict = OpenCcDict::new();
        Self::parse_characters_tsv(&mut dict, &chars_content);
        Self::parse_phrases_tsv(&mut dict, &phrases_content);
        Ok(dict)
    }

    /// 解析 `STCharacters.txt` 格式内容（每行：`简体字\t繁体1 [繁体2 ...]`）
    pub fn parse_characters_tsv(dict: &mut OpenCcDict, content: &str) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, vals)) = line.split_once('\t') else {
                continue;
            };
            let mut key_chars = key.trim().chars();
            let Some(simp_ch) = key_chars.next() else {
                continue;
            };
            if key_chars.next().is_some() {
                continue;
            }

            let mut trad_list = Vec::new();
            for part in vals.split_whitespace() {
                let mut p_chars = part.chars();
                if let Some(tc) = p_chars.next() {
                    if p_chars.next().is_none() && !trad_list.contains(&tc) {
                        trad_list.push(tc);
                    }
                }
            }
            dict.insert_char_mapping(simp_ch, trad_list);
        }
    }

    /// 解析 `STPhrases.txt` 格式内容（每行：`简体短语\t繁体短语1 [繁体短语2 ...]`）
    pub fn parse_phrases_tsv(dict: &mut OpenCcDict, content: &str) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, vals)) = line.split_once('\t') else {
                continue;
            };
            let simp = key.trim();
            let Some(first_trad) = vals.split_whitespace().next() else {
                continue;
            };
            if !simp.is_empty() && !first_trad.is_empty() {
                dict.insert_phrase_mapping(simp.to_string(), first_trad.to_string());
            }
        }
    }
}
