use crate::error::AlgoError;
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

/// 纯 Rust 简繁转换词库与正向最大匹配（FMM）引擎
///
/// # 设计原理
/// - **两级词表架构**：
///   1. `STCharacters`（单字规范表）：记录简化字到首选繁体字及多繁备选字的映射（如 `发 -> [發, 髮]`、`后 -> [後, 后]`）；
///   2. `STPhrases`（一简对多繁短语消歧表）：记录多字词组的精确繁体映射及边界保护词条（如 `头发 -> 頭髮`、`皇后 -> 皇后`）。
/// - **正向最大匹配（FMM）**：
///   转换多字词或句子时，利用 `first_char_max_len` 首字最大词长索引，优先从最长可能窗口向下执行零拷贝 UTF-8 切片查表；
///   命中的短语整体替换为对应繁体词并跨越游标，未命中的单字回退查询 `STCharacters` 首选繁体字。
/// - **单字候选一简多繁展开**：
///   当候选词为单个汉字且存在多个繁体映射时，首选字原位替换，次选繁体字自动派生为紧随其后的候选词，兼顾词组消歧与单字完备性。
#[derive(Debug, Clone, Default)]
pub struct OpenCcDict {
    /// 简化字单字 -> 繁体字序列（首元素为默认首选繁体字，后续元素为一简多繁备选字）
    pub chars: HashMap<char, Vec<char>>,
    /// 简体短语 -> 繁体短语映射表
    pub phrases: HashMap<String, String>,
    /// 短语首字 -> 以该字开头的最长短语字符数（用于加速 FMM 剪枝）
    pub first_char_max_len: HashMap<char, usize>,
}

impl OpenCcDict {
    /// 创建空的简繁转换词典
    pub fn new() -> Self {
        Self::default()
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
        let builtin_chars: &[(char, &[char])] = &[
            ('国', &['國']),
            ('会', &['會']),
            ('发', &['發', '髮']),
            ('后', &['後', '后']),
            ('干', &['干', '乾', '幹']),
            ('面', &['面', '麪']),
            ('表', &['表', '錶']),
            ('里', &['裏', '里']),
            ('历', &['歷', '曆']),
            ('复', &['復', '複']),
            ('系', &['系', '係', '繫']),
            ('学', &['學']),
            ('汉', &['漢']),
            ('语', &['語']),
            ('这', &['這']),
            ('来', &['來']),
            ('个', &['個']),
            ('们', &['們']),
            ('对', &['對']),
            ('经', &['經']),
            ('时', &['時']),
            ('现', &['現']),
            ('动', &['動']),
            ('进', &['進']),
            ('长', &['長']),
            ('问', &['問']),
            ('题', &['題']),
            ('见', &['見']),
            ('机', &['機']),
            ('点', &['點']),
            ('电', &['電']),
            ('华', &['華']),
            ('为', &['為']),
            ('头', &['頭']),
            ('条', &['條']),
            ('钟', &['鐘', '鍾']),
            ('净', &['淨']),
            ('燥', &['燥']),
            ('饼', &['餅']),
        ];
        for &(simp, trads) in builtin_chars {
            dict.insert_char_mapping(simp, trads.to_vec());
        }

        let builtin_phrases: &[(&str, &str)] = &[
            ("头发", "頭髮"),
            ("理发", "理髮"),
            ("白发", "白髮"),
            ("洗发", "洗髮"),
            ("假发", "假髮"),
            ("发廊", "髮廊"),
            ("皇后", "皇后"),
            ("太后", "太后"),
            ("王后", "王后"),
            ("面条", "麪條"),
            ("方便面", "方便麪"),
            ("牛肉面", "牛肉麪"),
            ("拉面", "拉麪"),
            ("面包", "麪包"),
            ("手表", "手錶"),
            ("钟表", "鐘錶"),
            ("干净", "乾淨"),
            ("干燥", "乾燥"),
            ("饼干", "餅乾"),
            ("干部", "幹部"),
            ("骨干", "骨幹"),
            ("历史", "歷史"),
            ("日历", "日曆"),
            ("农历", "農曆"),
            ("复习", "複習"),
            ("复杂", "複雜"),
            ("恢复", "恢復"),
        ];
        for &(simp, trad) in builtin_phrases {
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

    /// 从 OpenCC 标准 TSV 文本文件（`STCharacters.txt` 与 `STPhrases.txt`）解析构建词典
    pub fn from_tsv_files(
        chars_path: impl AsRef<Path>,
        phrases_path: impl AsRef<Path>,
    ) -> Result<Self, AlgoError> {
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

        let mut dict = Self::new();
        dict.parse_characters_tsv(&chars_content);
        dict.parse_phrases_tsv(&phrases_content);
        Ok(dict)
    }

    /// 解析 `STCharacters.txt` 格式内容（每行：`简体字\t繁体1 [繁体2 ...]`）
    pub fn parse_characters_tsv(&mut self, content: &str) {
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
            self.insert_char_mapping(simp_ch, trad_list);
        }
    }

    /// 解析 `STPhrases.txt` 格式内容（每行：`简体短语\t繁体短语1 [繁体短语2 ...]`）
    pub fn parse_phrases_tsv(&mut self, content: &str) {
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
                self.insert_phrase_mapping(simp.to_string(), first_trad.to_string());
            }
        }
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
            let len = bytes.len() as u16;
            string_pool.extend_from_slice(bytes);
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
        let mut sorted_phrases: Vec<(&String, &String)> = self.phrases.iter().collect();
        sorted_phrases.sort_by(|a, b| a.0.cmp(b.0));

        let mut phrase_records = Vec::with_capacity(sorted_phrases.len());
        for (simp, trad) in sorted_phrases {
            let (s_off, s_len) = intern(simp);
            let (t_off, t_len) = intern(trad);
            phrase_records.push((s_off, s_len, t_off, t_len));
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

        let char_count = u32::from_le_bytes(buf[16..20].try_into().unwrap()) as usize;
        let phrase_count = u32::from_le_bytes(buf[20..24].try_into().unwrap()) as usize;
        let pool_len = u32::from_le_bytes(buf[24..28].try_into().unwrap()) as usize;

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
            phrases: HashMap::with_capacity(phrase_count),
            first_char_max_len: HashMap::new(),
        };

        let mut cursor = 28usize;
        for _ in 0..char_count {
            let code = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap());
            let t_off = u32::from_le_bytes(buf[cursor + 4..cursor + 8].try_into().unwrap());
            let t_len = u16::from_le_bytes(buf[cursor + 8..cursor + 10].try_into().unwrap());
            cursor += 10;

            if let Some(simp_ch) = char::from_u32(code) {
                let trad_str = read_str(t_off, t_len)?;
                let trads: Vec<char> = trad_str.chars().collect();
                dict.insert_char_mapping(simp_ch, trads);
            }
        }

        for _ in 0..phrase_count {
            let s_off = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap());
            let s_len = u16::from_le_bytes(buf[cursor + 4..cursor + 6].try_into().unwrap());
            let t_off = u32::from_le_bytes(buf[cursor + 6..cursor + 10].try_into().unwrap());
            let t_len = u16::from_le_bytes(buf[cursor + 10..cursor + 12].try_into().unwrap());
            cursor += 12;

            let simp = read_str(s_off, s_len)?;
            let trad = read_str(t_off, t_len)?;
            dict.insert_phrase_mapping(simp.to_string(), trad.to_string());
        }

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
                    // 收集从第 2 个字符到 max_len 个字符的 UTF-8 字节结束偏移
                    let mut end_offsets: Vec<usize> = Vec::with_capacity(max_len);
                    for (char_idx, (byte_idx, ch)) in chars_iter.enumerate() {
                        let cur_char_count = char_idx + 2;
                        end_offsets.push(byte_idx + ch.len_utf8());
                        if cur_char_count >= max_len {
                            break;
                        }
                    }

                    // 从最长窗口向下递减执行零拷贝切片匹配
                    for &byte_end in end_offsets.iter().rev() {
                        let slice = &rem[..byte_end];
                        if let Some(trad_phrase) = self.phrases.get(slice) {
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
            let first_opt = char_iter.next();
            let is_single_char = first_opt.is_some() && char_iter.next().is_none();

            if is_single_char {
                let simp_ch = first_opt.unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmm_disambiguation_phrases_and_chars() {
        let dict = OpenCcDict::global();

        // 一简对多繁词组消歧验证
        assert_eq!(dict.convert_text("头发"), "頭髮");
        assert_eq!(dict.convert_text("理发"), "理髮");
        assert_eq!(dict.convert_text("发展"), "發展");
        assert_eq!(dict.convert_text("皇后"), "皇后");
        assert_eq!(dict.convert_text("以后"), "以後");
        assert_eq!(dict.convert_text("面条"), "麪條");
        assert_eq!(dict.convert_text("牛肉面"), "牛肉麪");
        assert_eq!(dict.convert_text("手表"), "手錶");
        assert_eq!(dict.convert_text("干净"), "乾淨");
        assert_eq!(dict.convert_text("干部"), "幹部");
    }

    #[test]
    fn test_single_char_multi_traditional_expansion() {
        let dict = OpenCcDict::global();
        let mut cands = vec![
            AlgoCandidate::new("头发", "tou'fa", 1000),
            AlgoCandidate::new("发", "fa", 800),
            AlgoCandidate::new("后", "hou", 700),
        ];

        dict.convert_candidates(&mut cands);
        let texts: Vec<&str> = cands.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts[0], "頭髮");
        assert!(texts.contains(&"發"), "单字候选 发 应包含首选繁体 發");
        assert!(texts.contains(&"髮"), "单字候选 发 应展开备选繁体 髮");
        assert!(texts.contains(&"後"), "单字候选 后 应包含首选繁体 後");
        assert!(texts.contains(&"后"), "单字候选 后 应展开备选繁体 后");
    }

    #[test]
    fn test_opencc_binary_save_and_load_roundtrip() {
        let dict = OpenCcDict::with_builtin_fallback();
        let temp_path = std::env::temp_dir().join("waylan_test_opencc.dict.bin");
        dict.save_to_file(&temp_path).expect("保存简繁二进制词库应成功");

        let loaded = OpenCcDict::load_from_file(&temp_path).expect("加载简繁二进制词库应成功");
        assert_eq!(loaded.convert_text("头发和牛肉面"), "頭髮和牛肉麪");
        assert_eq!(loaded.convert_text("皇后的手表"), "皇后的手錶");

        let _ = fs::remove_file(&temp_path);
    }
}
