//! 辅助混输与特殊符号二进制词库模型
//!
//! 提供英文单词补全、Unicode 符号表情、颜文字等混输词库的紧凑二进制编解码与安全原子落盘。

use crate::error::AlgoError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 辅助混输词库魔数标识（16 字节）
pub const AUX_DICT_MAGIC: &[u8; 16] = b"WAYLAN_AUXCOMP01";

/// 辅助与混输二进制词库结构（包含英文混输词表、符号与表情、颜文字特殊符号）
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AuxiliaryDict {
    /// 英文词目与词频序列 (如 ("rust", 100000))
    pub english_words: Vec<(String, u32)>,
    /// 符号与表情映射序列 (拼音语义, Unicode码点转义, 中文注释)
    pub symbols: Vec<(String, String, String)>,
}

impl AuxiliaryDict {
    /// 构建带有系统内置默认全量保底词库的实例
    pub fn with_default_data() -> Self {
        Self {
            english_words: Self::default_english_words(),
            symbols: Self::default_symbols(),
        }
    }

    /// 获取默认英文高频词汇表
    pub fn default_english_words() -> Vec<(String, u32)> {
        super::data::DEFAULT_ENGLISH_WORDS
            .iter()
            .enumerate()
            .map(|(idx, w)| (w.to_string(), 100_000_u32.saturating_sub(idx as u32 * 100)))
            .collect()
    }

    /// 获取默认 Unicode 符号与表情映射表
    pub fn default_symbols() -> Vec<(String, String, String)> {
        super::data::DEFAULT_SYMBOLS
            .iter()
            .map(|(py, sym, desc)| (py.to_string(), sym.to_string(), desc.to_string()))
            .collect()
    }

    /// 持久化保存为紧凑二进制词库文件 (WAYLAN_AUXCOMP01，采用临时文件+原子rename落盘防损坏)
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), AlgoError> {
        let path_ref = path.as_ref();
        if let Some(parent) = path_ref.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        let temp_path = path_ref.with_extension(format!("aux.{}.{}.tmp", pid, nanos));

        struct TempFileGuard<'a>(&'a Path, bool);
        impl<'a> Drop for TempFileGuard<'a> {
            fn drop(&mut self) {
                if !self.1 {
                    let _ = fs::remove_file(self.0);
                }
            }
        }
        let mut guard = TempFileGuard(&temp_path, false);

        let file = fs::File::create(&temp_path).map_err(|e| AlgoError::DictionaryIo {
            path: temp_path.clone(),
            source: e,
        })?;
        let mut writer = std::io::BufWriter::new(file);

        let mut interner = StringPoolInterner::default();
        let en_buf = encode_english_words(&self.english_words, &mut interner);
        let sym_buf = encode_symbols(&self.symbols, &mut interner);

        let payload = AuxDictPayload {
            num_english: self.english_words.len() as u32,
            num_symbols: self.symbols.len() as u32,
            en_buf: &en_buf,
            sym_buf: &sym_buf,
            string_pool: &interner.pool,
        };

        write_dict_data(&mut writer, path_ref, &payload)?;

        writer.flush().map_err(|e| AlgoError::DictionaryIo {
            path: temp_path.clone(),
            source: e,
        })?;
        let inner_file = writer.into_inner().map_err(|e| AlgoError::DictionaryIo {
            path: temp_path.clone(),
            source: e.into_error(),
        })?;
        inner_file.sync_all().map_err(|e| AlgoError::DictionaryIo {
            path: temp_path.clone(),
            source: e,
        })?;

        fs::rename(&temp_path, path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        // 成功完成原子重命名后解除守卫删除
        guard.1 = true;
        Ok(())
    }

    /// 从紧凑二进制文件加载 (强制魔数校验，杜绝隐式回退)
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let mut file = fs::File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let metadata = file.metadata().map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let file_len = metadata.len() as usize;

        let (num_english, num_symbols, meta_end) =
            read_and_validate_header(&mut file, path_ref, file_len)?;

        let mut en_buf = vec![0u8; num_english.saturating_mul(12)];
        file.read_exact(&mut en_buf)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        let mut sym_buf = vec![0u8; num_symbols.saturating_mul(18)];
        file.read_exact(&mut sym_buf)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        let string_pool = read_string_pool(&mut file, path_ref, meta_end, file_len)?;
        let english_words = decode_english_words(&en_buf, num_english, &string_pool);
        let symbols = decode_symbols(&sym_buf, num_symbols, &string_pool);

        Ok(Self {
            english_words,
            symbols,
        })
    }
}

/// 辅助词库构建期字符串驻留池管理器
#[derive(Default)]
struct StringPoolInterner {
    pool: Vec<u8>,
    offset_map: HashMap<String, (u32, u16)>,
}

impl StringPoolInterner {
    fn intern(&mut self, s: &str) -> (u32, u16) {
        if let Some(&loc) = self.offset_map.get(s) {
            return loc;
        }
        let off = self.pool.len() as u32;
        let bytes = s.as_bytes();
        let mut capped_len = bytes.len().min(u16::MAX as usize);
        while !s.is_char_boundary(capped_len) {
            capped_len -= 1;
        }
        let len = capped_len as u16;
        self.pool.extend_from_slice(&bytes[..capped_len]);
        self.offset_map.insert(s.to_string(), (off, len));
        (off, len)
    }
}

/// 编码英文混输词目索引段
fn encode_english_words(words: &[(String, u32)], interner: &mut StringPoolInterner) -> Vec<u8> {
    let mut buf = Vec::with_capacity(words.len() * 12);
    for (w, score) in words {
        let (off, len) = interner.intern(w);
        buf.extend_from_slice(&off.to_le_bytes());
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&score.to_le_bytes());
    }
    buf
}

/// 编码符号表情映射索引段
fn encode_symbols(
    symbols: &[(String, String, String)],
    interner: &mut StringPoolInterner,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(symbols.len() * 18);
    for (k, sym, desc) in symbols {
        let (k_off, k_len) = interner.intern(k);
        let (s_off, s_len) = interner.intern(sym);
        let (d_off, d_len) = interner.intern(desc);
        buf.extend_from_slice(&k_off.to_le_bytes());
        buf.extend_from_slice(&k_len.to_le_bytes());
        buf.extend_from_slice(&s_off.to_le_bytes());
        buf.extend_from_slice(&s_len.to_le_bytes());
        buf.extend_from_slice(&d_off.to_le_bytes());
        buf.extend_from_slice(&d_len.to_le_bytes());
    }
    buf
}

/// 辅助词库待落盘数据段负载载荷
struct AuxDictPayload<'a> {
    num_english: u32,
    num_symbols: u32,
    en_buf: &'a [u8],
    sym_buf: &'a [u8],
    string_pool: &'a [u8],
}

/// 写入词典魔数、统计头及数据段负载
fn write_dict_data(
    writer: &mut impl Write,
    path: &Path,
    payload: &AuxDictPayload<'_>,
) -> Result<(), AlgoError> {
    let io_err = |e| AlgoError::DictionaryIo {
        path: PathBuf::from(path),
        source: e,
    };
    writer.write_all(AUX_DICT_MAGIC).map_err(io_err)?;
    writer
        .write_all(&payload.num_english.to_le_bytes())
        .map_err(io_err)?;
    writer
        .write_all(&payload.num_symbols.to_le_bytes())
        .map_err(io_err)?;
    writer.write_all(&0u64.to_le_bytes()).map_err(io_err)?;
    writer.write_all(payload.en_buf).map_err(io_err)?;
    writer.write_all(payload.sym_buf).map_err(io_err)?;
    writer
        .write_all(&(payload.string_pool.len() as u32).to_le_bytes())
        .map_err(io_err)?;
    writer.write_all(payload.string_pool).map_err(io_err)?;
    Ok(())
}

/// 读取并校验词库文件头部与段边界
fn read_and_validate_header(
    file: &mut fs::File,
    path: &Path,
    file_len: usize,
) -> Result<(usize, usize, usize), AlgoError> {
    if file_len < 36 {
        return Err(AlgoError::deserialize_failed(
            path,
            "辅助词典文件体积过小或已损坏",
        ));
    }

    let mut magic = [0u8; 16];
    file.read_exact(&mut magic)
        .map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path),
            source: e,
        })?;
    if &magic != AUX_DICT_MAGIC {
        return Err(AlgoError::deserialize_failed(
            path,
            "无效的辅助词典魔数标识",
        ));
    }

    let mut header = [0u8; 16];
    file.read_exact(&mut header)
        .map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path),
            source: e,
        })?;
    let num_english = u32::from_le_bytes(header[0..4].try_into().unwrap_or([0; 4])) as usize;
    let num_symbols = u32::from_le_bytes(header[4..8].try_into().unwrap_or([0; 4])) as usize;

    let en_bytes = num_english.saturating_mul(12);
    let sym_bytes = num_symbols.saturating_mul(18);
    let meta_end = 32usize
        .saturating_add(en_bytes)
        .saturating_add(sym_bytes)
        .saturating_add(4);
    if meta_end > file_len {
        return Err(AlgoError::deserialize_failed(
            path,
            "辅助词典索引段长度超出文件边界，文件可能已损坏",
        ));
    }

    Ok((num_english, num_symbols, meta_end))
}

/// 读取并校验紧凑字符串池切片
fn read_string_pool(
    file: &mut fs::File,
    path: &Path,
    meta_end: usize,
    file_len: usize,
) -> Result<Vec<u8>, AlgoError> {
    let mut pool_len_buf = [0u8; 4];
    file.read_exact(&mut pool_len_buf)
        .map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path),
            source: e,
        })?;
    let pool_len = u32::from_le_bytes(pool_len_buf) as usize;
    if meta_end.saturating_add(pool_len) > file_len {
        return Err(AlgoError::deserialize_failed(
            path,
            "辅助词典字符串池长度超出文件边界，文件可能已损坏",
        ));
    }

    let mut string_pool = vec![0u8; pool_len];
    file.read_exact(&mut string_pool)
        .map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path),
            source: e,
        })?;
    Ok(string_pool)
}

/// 安全从字符串池提取 UTF-8 子字符串
#[inline]
fn slice_str_from_pool(pool: &[u8], off: usize, len: usize) -> String {
    let end = off.saturating_add(len);
    if end <= pool.len() {
        std::str::from_utf8(&pool[off..end])
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    }
}

/// 解码英文混输词目列表
fn decode_english_words(en_buf: &[u8], count: usize, pool: &[u8]) -> Vec<(String, u32)> {
    let mut words = Vec::with_capacity(count);
    for i in 0..count {
        let base = i * 12;
        let off = u32::from_le_bytes(en_buf[base..base + 4].try_into().unwrap_or([0; 4])) as usize;
        let len =
            u16::from_le_bytes(en_buf[base + 4..base + 6].try_into().unwrap_or([0; 2])) as usize;
        let score = u32::from_le_bytes(en_buf[base + 8..base + 12].try_into().unwrap_or([0; 4]));
        words.push((slice_str_from_pool(pool, off, len), score));
    }
    words
}

/// 解码符号表情映射列表
fn decode_symbols(sym_buf: &[u8], count: usize, pool: &[u8]) -> Vec<(String, String, String)> {
    let mut symbols = Vec::with_capacity(count);
    for i in 0..count {
        let base = i * 18;
        let k_off =
            u32::from_le_bytes(sym_buf[base..base + 4].try_into().unwrap_or([0; 4])) as usize;
        let k_len =
            u16::from_le_bytes(sym_buf[base + 4..base + 6].try_into().unwrap_or([0; 2])) as usize;
        let s_off =
            u32::from_le_bytes(sym_buf[base + 6..base + 10].try_into().unwrap_or([0; 4])) as usize;
        let s_len =
            u16::from_le_bytes(sym_buf[base + 10..base + 12].try_into().unwrap_or([0; 2])) as usize;
        let d_off =
            u32::from_le_bytes(sym_buf[base + 12..base + 16].try_into().unwrap_or([0; 4])) as usize;
        let d_len =
            u16::from_le_bytes(sym_buf[base + 16..base + 18].try_into().unwrap_or([0; 2])) as usize;
        symbols.push((
            slice_str_from_pool(pool, k_off, k_len),
            slice_str_from_pool(pool, s_off, s_len),
            slice_str_from_pool(pool, d_off, d_len),
        ));
    }
    symbols
}
