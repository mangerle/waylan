use crate::error::AlgoError;
use crate::opencc_compact::CompactPhraseTable;
use crate::types::AlgoCandidate;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// 简繁转换二进制词库魔数标识（16 字节）
pub const OPENCC_DICT_MAGIC: &[u8; 16] = b"WAYLAN_OPENCC001";

/// 全局共享默认简繁转换词典缓存
static GLOBAL_OPENCC_DICT: OnceLock<Arc<OpenCcDict>> = OnceLock::new();

/// 紧凑短语映射表（按 UTF-8 字节序连续存储，通过二分查找实现零堆分配极速查询）
#[derive(Debug, Clone, Default)]
pub struct OpenCcDict {
    /// 简化字单字 -> 繁体字序列（首元素为默认首选繁体字，后续元素为一简多繁备选字）
    pub chars: HashMap<char, Vec<char>>,
    /// 简体短语 -> 繁体短语映射表（动态构造与 TSV 构建阶段使用）
    pub phrases: HashMap<String, String>,
    /// 紧凑短语表（从二进制词库加载时常驻，提供零堆分配二分检索）
    pub compact_phrases: Option<CompactPhraseTable>,
    /// 短语首字 -> 以该字开头的最长短语字符数（用于加速 FMM 剪枝）
    pub first_char_max_len: HashMap<char, usize>,
}

impl OpenCcDict {
    /// 创建空的简繁转换词典
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取短语总条目数
    pub fn phrases_len(&self) -> usize {
        if let Some(ref compact) = self.compact_phrases {
            compact.len()
        } else {
            self.phrases.len()
        }
    }

    /// 查询指定简体短语对应的繁体短语（优先二分检索紧凑表，回退查询 HashMap）
    pub fn lookup_phrase(&self, simp: &str) -> Option<&str> {
        if let Some(ref compact) = self.compact_phrases {
            return compact.lookup(simp);
        }
        self.phrases.get(simp).map(|s| s.as_str())
    }

    /// 获取全局共享简繁转换词典实例（优先加载二进制 opencc.dict.bin，其次加载开源词表文本，最后使用内置核心表兜底）
    pub fn global() -> Arc<Self> {
        GLOBAL_OPENCC_DICT
            .get_or_init(|| Arc::new(Self::load_default()))
            .clone()
    }

    /// 按多级路径加载默认简繁词典
    pub fn load_default() -> Self {
        let bin_candidates = [
            PathBuf::from("data/opencc.dict.bin"),
            PathBuf::from("../../data/opencc.dict.bin"),
            PathBuf::from("/usr/share/waylan/data/opencc.dict.bin"),
            PathBuf::from("/usr/local/share/waylan/data/opencc.dict.bin"),
        ];
        for path in &bin_candidates {
            if path.exists() {
                if let Ok(dict) = Self::load_from_file(path) {
                    return dict;
                }
            }
        }

        let tsv_dirs = [
            PathBuf::from("data/dicts/8_opencc"),
            PathBuf::from("../../data/dicts/8_opencc"),
            PathBuf::from("/usr/share/waylan/dicts/8_opencc"),
        ];
        for dir in &tsv_dirs {
            let chars_path = dir.join("STCharacters.txt");
            let phrases_path = dir.join("STPhrases.txt");
            if chars_path.exists() && phrases_path.exists() {
                if let Ok(dict) = Self::from_tsv_files(&chars_path, &phrases_path) {
                    return dict;
                }
            }
        }

        Self::with_builtin_fallback()
    }

    /// 内置核心简繁单字与一简多繁短语兜底表（供无外部词库文件时的轻量环境使用）
    pub fn with_builtin_fallback() -> Self {
        let mut dict = Self::new();
        for &(simp, trads) in crate::opencc_data::BUILTIN_CHARS {
            dict.insert_char_mapping(simp, trads.to_vec());
        }
        for &(simp, trad) in crate::opencc_data::BUILTIN_PHRASES {
            dict.insert_phrase_mapping(simp.to_string(), trad.to_string());
        }

        dict
    }

    /// 插入一条单字简繁映射
    pub fn insert_char_mapping(&mut self, simp: char, trads: Vec<char>) {
        if !trads.is_empty() {
            self.chars.insert(simp, trads);
        }
    }

    /// 插入一条短语简繁映射并更新首字最大词长索引
    pub fn insert_phrase_mapping(&mut self, simp: String, trad: String) {
        let mut char_iter = simp.chars();
        if let Some(first_ch) = char_iter.next() {
            let char_len = 1 + char_iter.count();
            if char_len >= 2 {
                let entry = self.first_char_max_len.entry(first_ch).or_insert(0);
                if char_len > *entry {
                    *entry = char_len;
                }
                self.phrases.insert(simp, trad);
            }
        }
    }

    /// 从 OpenCC 标准 TSV 文本文件解析构建词典
    #[inline]
    pub fn from_tsv_files(
        chars_path: impl AsRef<Path>,
        phrases_path: impl AsRef<Path>,
    ) -> Result<Self, AlgoError> {
        crate::opencc_tsv::OpenCcTsvParser::from_tsv_files(chars_path, phrases_path)
    }

    /// 解析 `STCharacters.txt` 格式内容
    #[inline]
    pub fn parse_characters_tsv(&mut self, content: &str) {
        crate::opencc_tsv::OpenCcTsvParser::parse_characters_tsv(self, content);
    }

    /// 解析 `STPhrases.txt` 格式内容
    #[inline]
    pub fn parse_phrases_tsv(&mut self, content: &str) {
        crate::opencc_tsv::OpenCcTsvParser::parse_phrases_tsv(self, content);
    }

    /// 编译并持久化保存为紧凑二进制词库文件 (`WAYLAN_OPENCC001`)
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), AlgoError> {
        let path_ref = path.as_ref();
        if let Some(parent) = path_ref.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = fs::File::create(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let mut writer = std::io::BufWriter::new(file);

        let mut string_pool = Vec::new();
        let mut pool_map: HashMap<String, (u32, u16)> = HashMap::new();

        let mut intern = |s: &str| -> (u32, u16) {
            if let Some(&loc) = pool_map.get(s) {
                return loc;
            }
            let off = string_pool.len() as u32;
            let bytes = s.as_bytes();
            let capped_len = bytes.len().min(u16::MAX as usize);
            let len = capped_len as u16;
            string_pool.extend_from_slice(&bytes[..capped_len]);
            pool_map.insert(s.to_string(), (off, len));
            (off, len)
        };

        // 1. 整理有序单字条目
        let mut sorted_chars: Vec<(char, &Vec<char>)> =
            self.chars.iter().map(|(&k, v)| (k, v)).collect();
        sorted_chars.sort_by_key(|(k, _)| *k);

        let mut char_records = Vec::with_capacity(sorted_chars.len());
        for (simp_ch, trads) in sorted_chars {
            let joined: String = trads.iter().collect();
            let (t_off, t_len) = intern(&joined);
            char_records.push((simp_ch as u32, t_off, t_len));
        }

        // 2. 整理有序短语条目
        let mut phrase_records = Vec::new();
        if !self.phrases.is_empty() {
            let mut sorted_phrases: Vec<(&String, &String)> = self.phrases.iter().collect();
            sorted_phrases.sort_by(|a, b| a.0.cmp(b.0));
            phrase_records.reserve(sorted_phrases.len());
            for (simp, trad) in sorted_phrases {
                let (s_off, s_len) = intern(simp);
                let (t_off, t_len) = intern(trad);
                phrase_records.push((s_off, s_len, t_off, t_len));
            }
        } else if let Some(ref compact) = self.compact_phrases {
            phrase_records.reserve(compact.records.len());
            for &(s_off, s_len, t_off, t_len) in &compact.records {
                let s_start = s_off as usize;
                let s_end = s_start.saturating_add(s_len as usize);
                let t_start = t_off as usize;
                let t_end = t_start.saturating_add(t_len as usize);
                let s_str = compact
                    .pool
                    .get(s_start..s_end)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                let t_str = compact
                    .pool
                    .get(t_start..t_end)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                let (ns_off, ns_len) = intern(s_str);
                let (nt_off, nt_len) = intern(t_str);
                phrase_records.push((ns_off, ns_len, nt_off, nt_len));
            }
        }

        // 3. 写入头部与数据表
        let io_err = |e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        };
        writer.write_all(OPENCC_DICT_MAGIC).map_err(io_err)?;
        writer
            .write_all(&(char_records.len() as u32).to_le_bytes())
            .map_err(io_err)?;
        writer
            .write_all(&(phrase_records.len() as u32).to_le_bytes())
            .map_err(io_err)?;
        writer
            .write_all(&(string_pool.len() as u32).to_le_bytes())
            .map_err(io_err)?;

        for (code, t_off, t_len) in char_records {
            writer.write_all(&code.to_le_bytes()).map_err(io_err)?;
            writer.write_all(&t_off.to_le_bytes()).map_err(io_err)?;
            writer.write_all(&t_len.to_le_bytes()).map_err(io_err)?;
        }

        for (s_off, s_len, t_off, t_len) in phrase_records {
            writer.write_all(&s_off.to_le_bytes()).map_err(io_err)?;
            writer.write_all(&s_len.to_le_bytes()).map_err(io_err)?;
            writer.write_all(&t_off.to_le_bytes()).map_err(io_err)?;
            writer.write_all(&t_len.to_le_bytes()).map_err(io_err)?;
        }

        writer.write_all(&string_pool).map_err(io_err)?;
        writer.flush().map_err(io_err)?;
        Ok(())
    }

    /// 从紧凑二进制文件 (`WAYLAN_OPENCC001`) 加载简繁词典
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let mut file = fs::File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        let invalid_data = |msg: &str| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, msg),
        };

        if buf.len() < 28 || &buf[0..16] != OPENCC_DICT_MAGIC {
            return Err(invalid_data("简繁词库文件魔数或头部长度不合法"));
        }

        let char_count = u32::from_le_bytes(buf[16..20].try_into().unwrap_or([0; 4])) as usize;
        let phrase_count = u32::from_le_bytes(buf[20..24].try_into().unwrap_or([0; 4])) as usize;
        let pool_len = u32::from_le_bytes(buf[24..28].try_into().unwrap_or([0; 4])) as usize;

        let chars_bytes = char_count.saturating_mul(10);
        let phrases_bytes = phrase_count.saturating_mul(12);
        let pool_start = 28usize
            .saturating_add(chars_bytes)
            .saturating_add(phrases_bytes);

        if buf.len() < pool_start.saturating_add(pool_len) {
            return Err(invalid_data("简繁词库文件数据区或字符串池截断"));
        }

        let pool = &buf[pool_start..pool_start + pool_len];
        let read_str = |off: u32, len: u16| -> Result<&str, AlgoError> {
            let s = off as usize;
            let e = s.saturating_add(len as usize);
            if e > pool.len() {
                return Err(invalid_data("简繁词库字符串池偏移越界"));
            }
            std::str::from_utf8(&pool[s..e])
                .map_err(|_| invalid_data("简繁词库字符串池包含非法 UTF-8 序列"))
        };

        let mut dict = Self {
            chars: HashMap::with_capacity(char_count),
            phrases: HashMap::new(),
            compact_phrases: None,
            first_char_max_len: HashMap::new(),
        };

        let mut cursor = 28usize;
        for _ in 0..char_count {
            let code = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap_or([0; 4]));
            let t_off =
                u32::from_le_bytes(buf[cursor + 4..cursor + 8].try_into().unwrap_or([0; 4]));
            let t_len =
                u16::from_le_bytes(buf[cursor + 8..cursor + 10].try_into().unwrap_or([0; 2]));
            cursor += 10;

            if let Some(simp_ch) = char::from_u32(code) {
                let trad_str = read_str(t_off, t_len)?;
                let trads: Vec<char> = trad_str.chars().collect();
                dict.insert_char_mapping(simp_ch, trads);
            }
        }

        let mut phrase_records = Vec::with_capacity(phrase_count);
        for _ in 0..phrase_count {
            let s_off = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap_or([0; 4]));
            let s_len =
                u16::from_le_bytes(buf[cursor + 4..cursor + 6].try_into().unwrap_or([0; 2]));
            let t_off =
                u32::from_le_bytes(buf[cursor + 6..cursor + 10].try_into().unwrap_or([0; 4]));
            let t_len =
                u16::from_le_bytes(buf[cursor + 10..cursor + 12].try_into().unwrap_or([0; 2]));
            cursor += 12;

            phrase_records.push((s_off, s_len, t_off, t_len));

            let simp = read_str(s_off, s_len)?;
            if let Some(first_ch) = simp.chars().next() {
                let char_len = simp.chars().count();
                let entry = dict.first_char_max_len.entry(first_ch).or_insert(0);
                if char_len > *entry {
                    *entry = char_len;
                }
            }
        }

        dict.compact_phrases = Some(CompactPhraseTable {
            records: phrase_records,
            pool: pool.to_vec(),
        });

        Ok(dict)
    }

    /// 使用正向最大匹配（FMM）将输入简体文本转换为繁体文本
    pub fn convert_text(&self, text: &str) -> String {
        if text.is_empty() || text.is_ascii() {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        let mut pos = 0usize;

        while pos < text.len() {
            let rem = &text[pos..];
            let mut chars_iter = rem.char_indices();
            let Some((_, first_ch)) = chars_iter.next() else {
                break;
            };

            let mut matched_phrase = false;
            if let Some(&max_len) = self.first_char_max_len.get(&first_ch) {
                if max_len >= 2 {
                    // 使用栈固定数组收集 UTF-8 字节结束偏移，彻底消除每次字符匹配时的堆内存分配
                    const MAX_FMM_WINDOW: usize = 24;
                    let mut end_offsets = [0usize; MAX_FMM_WINDOW];
                    let mut offset_count = 0usize;
                    let limit = max_len.min(MAX_FMM_WINDOW);

                    for (char_idx, (byte_idx, ch)) in chars_iter.enumerate() {
                        let cur_char_count = char_idx + 2;
                        if offset_count < MAX_FMM_WINDOW {
                            end_offsets[offset_count] = byte_idx + ch.len_utf8();
                            offset_count += 1;
                        }
                        if cur_char_count >= limit {
                            break;
                        }
                    }

                    // 从最长窗口向下递减执行零拷贝切片匹配
                    for &byte_end in end_offsets[..offset_count].iter().rev() {
                        let slice = &rem[..byte_end];
                        if let Some(trad_phrase) = self.lookup_phrase(slice) {
                            result.push_str(trad_phrase);
                            pos += byte_end;
                            matched_phrase = true;
                            break;
                        }
                    }
                }
            }

            if !matched_phrase {
                let mapped_ch = self
                    .chars
                    .get(&first_ch)
                    .and_then(|list| list.first().copied())
                    .unwrap_or(first_ch);
                result.push(mapped_ch);
                pos += first_ch.len_utf8();
            }
        }

        result
    }

    /// 对候选词列表执行简转繁处理：
    /// - 多字词候选（字数 >= 2）：按 FMM 正向最大匹配唯一消歧转换；
    /// - 单字候选（字数 == 1）：首选繁体字原位替换，若存在一简对多繁备选字（如 发 -> 發/髮、后 -> 後/后），自动派生备选单字候选。
    pub fn convert_candidates(&self, candidates: &mut Vec<AlgoCandidate>) {
        let mut converted = Vec::with_capacity(candidates.len() + 4);

        for mut cand in candidates.drain(..) {
            let mut char_iter = cand.text.chars();
            if let (Some(simp_ch), None) = (char_iter.next(), char_iter.next()) {
                if let Some(trad_list) = self.chars.get(&simp_ch) {
                    if let Some(&primary) = trad_list.first() {
                        let base_cand = cand.clone();
                        cand.text = primary.to_string();
                        converted.push(cand);

                        for (idx, &alt_ch) in trad_list.iter().skip(1).enumerate() {
                            if alt_ch != primary {
                                let mut derived = base_cand.clone();
                                derived.text = alt_ch.to_string();
                                derived.score =
                                    base_cand.score.saturating_sub(((idx as u32) + 1) * 10);
                                converted.push(derived);
                            }
                        }
                        continue;
                    }
                }
                converted.push(cand);
            } else {
                cand.text = self.convert_text(&cand.text);
                if cand.is_expandable && !cand.sub_candidates.is_empty() {
                    for sub in &mut cand.sub_candidates {
                        *sub = self.convert_text(sub);
                    }
                }
                converted.push(cand);
            }
        }

        *candidates = converted;
    }
}
