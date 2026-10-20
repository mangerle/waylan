//! 简繁转换紧凑二进制词库 (`WAYLAN_OPENCC001`) 序列化与反序列化
//!
//! # 设计原理
//! - **实现初衷**：将 OpenCC 词典编译为零拷贝内存映射兼容的二进制词库格式。
//! - **核心优势**：关注点分离，将字符串池 Interning、单字表序列化与短语表反序列化解耦为细粒度单一职责函数。
//! - **复杂度控制**：各独立处理函数严格限制在 50 行以内，消除百行单体方法。

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::compact::CompactPhraseTable;
use super::{OpenCcDict, OPENCC_DICT_MAGIC};
use crate::error::AlgoError;

/// 字符串驻留池（负责去重、字符边界安全截断与紧凑字节累积）
#[derive(Default)]
struct OpenCcStringPool {
    pool: Vec<u8>,
    indices: HashMap<String, (u32, u16)>,
}

impl OpenCcStringPool {
    /// 将字符串写入字符串池并返回 (offset, len)
    fn intern(&mut self, s: &str) -> (u32, u16) {
        if let Some(&loc) = self.indices.get(s) {
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
        self.indices.insert(s.to_string(), (off, len));
        (off, len)
    }

    #[inline]
    fn as_slice(&self) -> &[u8] {
        &self.pool
    }

    #[inline]
    fn len(&self) -> usize {
        self.pool.len()
    }
}

/// 简繁二进制词库写入参数对象
///
/// 承载写入紧凑二进制词库所需的全部上下文：二进制输出流、错误定位路径、单字与短语记录表以及字符串驻留池。
///
/// 设计原理：写入流程所需的入参天然构成一个整体，聚合为参数对象可避免长参数列表在调用处错位，
/// 同时使各字段的语义边界一目了然。
struct OpenccRecordWriteParams<'a, W: Write> {
    /// 二进制输出流（输出槽位）
    writer: &'a mut W,
    /// 词库文件路径（用于构造带路径上下文的 IO 错误）
    path_ref: &'a Path,
    /// 单字记录表（码位、繁体串偏移与长度）
    char_records: &'a [(u32, u32, u16)],
    /// 短语记录表（简体串偏移与长度、繁体串偏移与长度）
    phrase_records: &'a [(u32, u16, u32, u16)],
    /// 字符串驻留池字节数据
    string_pool: &'a [u8],
    /// 字符串驻留池字节长度
    pool_len: usize,
}

/// 简繁二进制词库单字记录解析参数对象
///
/// 承载解析单字记录表所需的缓冲区上下文、记录条数、字符串驻留池与待填充词典实例。
struct OpenccCharParseParams<'a> {
    /// 词库原始字节缓冲区
    buf: &'a [u8],
    /// 单字记录条数
    char_count: usize,
    /// 字符串驻留池字节数据
    pool: &'a [u8],
    /// 待填充的词典实例（输出槽位）
    dict: &'a mut OpenCcDict,
    /// 词库文件路径（用于构造带路径上下文的 IO 错误）
    path_ref: &'a Path,
}

/// 简繁二进制词库短语记录解析参数对象
///
/// 承载解析短语记录表所需的缓冲区上下文、记录条数、解析游标、字符串驻留池与待填充词典实例。
struct OpenccPhraseParseParams<'a> {
    /// 词库原始字节缓冲区
    buf: &'a [u8],
    /// 短语记录条数
    phrase_count: usize,
    /// 记录区解析游标（单字记录表解析后的起始偏移）
    cursor: usize,
    /// 字符串驻留池字节数据
    pool: &'a [u8],
    /// 待填充的词典实例（输出槽位）
    dict: &'a mut OpenCcDict,
    /// 词库文件路径（用于构造带路径上下文的 IO 错误）
    path_ref: &'a Path,
}

impl OpenCcDict {
    /// 整理有序单字记录条目
    fn build_char_records(&self, pool: &mut OpenCcStringPool) -> Vec<(u32, u32, u16)> {
        let mut sorted_chars: Vec<(char, &Vec<char>)> =
            self.chars.iter().map(|(&k, v)| (k, v)).collect();
        sorted_chars.sort_by_key(|(k, _)| *k);

        let mut char_records = Vec::with_capacity(sorted_chars.len());
        for (simp_ch, trads) in sorted_chars {
            let joined: String = trads.iter().collect();
            let (t_off, t_len) = pool.intern(&joined);
            char_records.push((simp_ch as u32, t_off, t_len));
        }
        char_records
    }

    /// 整理有序短语记录条目
    fn build_phrase_records(&self, pool: &mut OpenCcStringPool) -> Vec<(u32, u16, u32, u16)> {
        let mut phrase_records = Vec::new();
        if !self.phrases.is_empty() {
            let mut sorted_phrases: Vec<(&String, &String)> = self.phrases.iter().collect();
            sorted_phrases.sort_by(|a, b| a.0.cmp(b.0));
            phrase_records.reserve(sorted_phrases.len());
            for (simp, trad) in sorted_phrases {
                let (s_off, s_len) = pool.intern(simp);
                let (t_off, t_len) = pool.intern(trad);
                phrase_records.push((s_off, s_len, t_off, t_len));
            }
        } else if let Some(ref compact) = self.compact_phrases {
            phrase_records.reserve(compact.records.len());
            for &(s_off, s_len, t_off, t_len) in &compact.records {
                let s_str = compact.resolve_str(s_off, s_len);
                let t_str = compact.resolve_str(t_off, t_len);
                let (ns_off, ns_len) = pool.intern(s_str);
                let (nt_off, nt_len) = pool.intern(t_str);
                phrase_records.push((ns_off, ns_len, nt_off, nt_len));
            }
        }
        phrase_records
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
        let mut string_pool = OpenCcStringPool::default();

        let char_records = self.build_char_records(&mut string_pool);
        let phrase_records = self.build_phrase_records(&mut string_pool);

        write_opencc_records(OpenccRecordWriteParams {
            writer: &mut writer,
            path_ref,
            char_records: &char_records,
            phrase_records: &phrase_records,
            string_pool: string_pool.as_slice(),
            pool_len: string_pool.len(),
        })
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

        let (char_count, phrase_count, pool_start, pool_end) = parse_opencc_header(&buf, path_ref)?;
        let pool = &buf[pool_start..pool_end];

        let mut dict = Self {
            chars: HashMap::with_capacity(char_count),
            phrases: HashMap::new(),
            compact_phrases: None,
            first_char_max_len: HashMap::new(),
        };

        let phrase_offset = parse_char_records(OpenccCharParseParams {
            buf: &buf,
            char_count,
            pool,
            dict: &mut dict,
            path_ref,
        })?;
        parse_phrase_records(OpenccPhraseParseParams {
            buf: &buf,
            phrase_count,
            cursor: phrase_offset,
            pool,
            dict: &mut dict,
            path_ref,
        })?;

        Ok(dict)
    }
}

/// 写入头部与记录数组
fn write_opencc_records<W: Write>(params: OpenccRecordWriteParams<'_, W>) -> Result<(), AlgoError> {
    let OpenccRecordWriteParams {
        writer,
        path_ref,
        char_records,
        phrase_records,
        string_pool,
        pool_len,
    } = params;

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
        .write_all(&(pool_len as u32).to_le_bytes())
        .map_err(io_err)?;

    for &(code, t_off, t_len) in char_records {
        writer.write_all(&code.to_le_bytes()).map_err(io_err)?;
        writer.write_all(&t_off.to_le_bytes()).map_err(io_err)?;
        writer.write_all(&t_len.to_le_bytes()).map_err(io_err)?;
    }

    for &(s_off, s_len, t_off, t_len) in phrase_records {
        writer.write_all(&s_off.to_le_bytes()).map_err(io_err)?;
        writer.write_all(&s_len.to_le_bytes()).map_err(io_err)?;
        writer.write_all(&t_off.to_le_bytes()).map_err(io_err)?;
        writer.write_all(&t_len.to_le_bytes()).map_err(io_err)?;
    }

    writer.write_all(string_pool).map_err(io_err)?;
    writer.flush().map_err(io_err)?;
    Ok(())
}

/// 校验并解析 OpenCC 二进制文件头部结构
fn parse_opencc_header(
    buf: &[u8],
    path_ref: &Path,
) -> Result<(usize, usize, usize, usize), AlgoError> {
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

    Ok((char_count, phrase_count, pool_start, pool_end))
}

/// 解析单字记录表并填充词典映射
fn parse_char_records(params: OpenccCharParseParams<'_>) -> Result<usize, AlgoError> {
    let OpenccCharParseParams {
        buf,
        char_count,
        pool,
        dict,
        path_ref,
    } = params;

    let mut cursor = 28usize;
    for _ in 0..char_count {
        let code = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap_or([0; 4]));
        let t_off = u32::from_le_bytes(buf[cursor + 4..cursor + 8].try_into().unwrap_or([0; 4]));
        let t_len = u16::from_le_bytes(buf[cursor + 8..cursor + 10].try_into().unwrap_or([0; 2]));
        cursor = cursor.saturating_add(10);

        if let Some(simp_ch) = char::from_u32(code) {
            let trad_str = read_pool_str(pool, t_off, t_len, path_ref)?;
            let trads: Vec<char> = trad_str.chars().collect();
            dict.insert_char_mapping(simp_ch, trads);
        }
    }
    Ok(cursor)
}

/// 解析短语记录表并填充 CompactPhraseTable
fn parse_phrase_records(params: OpenccPhraseParseParams<'_>) -> Result<(), AlgoError> {
    let OpenccPhraseParseParams {
        buf,
        phrase_count,
        mut cursor,
        pool,
        dict,
        path_ref,
    } = params;

    let mut phrase_records = Vec::with_capacity(phrase_count);
    for _ in 0..phrase_count {
        let s_off = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap_or([0; 4]));
        let s_len = u16::from_le_bytes(buf[cursor + 4..cursor + 6].try_into().unwrap_or([0; 2]));
        let t_off = u32::from_le_bytes(buf[cursor + 6..cursor + 10].try_into().unwrap_or([0; 4]));
        let t_len = u16::from_le_bytes(buf[cursor + 10..cursor + 12].try_into().unwrap_or([0; 2]));
        cursor = cursor.saturating_add(12);

        let simp = read_pool_str(pool, s_off, s_len, path_ref)?;
        let _ = read_pool_str(pool, t_off, t_len, path_ref)?;
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

    Ok(())
}

/// 从字符串池切片安全读取 UTF-8 字符串
fn read_pool_str<'a>(
    pool: &'a [u8],
    off: u32,
    len: u16,
    path_ref: &Path,
) -> Result<&'a str, AlgoError> {
    let invalid_data = |msg: &str| AlgoError::DictionaryIo {
        path: PathBuf::from(path_ref),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, msg),
    };
    let s = off as usize;
    let e = s.saturating_add(len as usize);
    if e > pool.len() {
        return Err(invalid_data("简繁词库字符串池偏移越界"));
    }
    std::str::from_utf8(&pool[s..e])
        .map_err(|_| invalid_data("简繁词库字符串池包含非法 UTF-8 序列"))
}

impl CompactPhraseTable {
    /// 辅助方法：从内部池安全提取字符串
    fn resolve_str(&self, off: u32, len: u16) -> &str {
        let s = off as usize;
        let e = s.saturating_add(len as usize);
        self.pool
            .get(s..e)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("")
    }
}
