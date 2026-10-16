//! 简繁转换紧凑二进制词库 (`WAYLAN_OPENCC001`) 序列化与反序列化

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::{OpenCcDict, OPENCC_DICT_MAGIC};
use crate::error::AlgoError;
use crate::opencc_compact::CompactPhraseTable;

impl OpenCcDict {
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
            let mut capped_len = bytes.len().min(u16::MAX as usize);
            while !s.is_char_boundary(capped_len) {
                capped_len -= 1;
            }
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

        let pool_end = pool_start.saturating_add(pool_len);
        if buf.len() < pool_end {
            return Err(invalid_data("简繁词库文件数据区或字符串池截断"));
        }

        let pool = &buf[pool_start..pool_end];
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
            cursor = cursor.saturating_add(10);

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
            cursor = cursor.saturating_add(12);

            let simp = read_str(s_off, s_len)?;
            let _ = read_str(t_off, t_len)?;
            phrase_records.push((s_off, s_len, t_off, t_len));

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
}
