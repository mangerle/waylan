use crate::error::AlgoError;
use crate::types::{AlgoCandidate, CandidateTier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 用户自定义短语管理器
///
/// # 设计原理
/// - 支持用户在纯文本文件 (~/.config/waylan/phrases.txt) 中维护个性化快捷短语。
/// - 格式支持 "缩写=展开短语" 或 "缩写 展开短语"（如 mymail=abc@domain.com）。
/// - 检索耗时 O(1)，匹配即刻置顶输出。
#[derive(Debug, Default, Clone)]
pub struct CustomPhraseManager {
    /// 缩写代码到短语列表的映射
    phrases: HashMap<String, Vec<String>>,
}

impl CustomPhraseManager {
    pub fn new() -> Self {
        Self {
            phrases: HashMap::new(),
        }
    }

    /// 获取默认自定义短语配置文件路径
    pub fn default_path() -> PathBuf {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".config")
                .join("waylan")
                .join("phrases.txt")
        } else {
            PathBuf::from("phrases.txt")
        }
    }

    /// 从文件加载自定义短语，若不存在可自动创建模板
    pub fn load_or_init(path: &Path) -> Self {
        let mut manager = Self::new();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                manager.parse_content(&content);
            }
        } else {
            // 初始化带有常用示例的模板文件
            let template = "# 微澜输入法自定义短语配置文件\n\
                            # 格式：快捷拼音/代码=上屏内容（每行一条）\n\
                            mymail=user@example.com\n\
                            myphone=13800138000\n\
                            addr=北京市海淀区中关村大街1号\n";
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, template);
            manager.parse_content(template);
        }
        manager
    }

    /// 解析文本内容
    pub fn parse_content(&mut self, content: &str) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((code, text)) = line.split_once('=') {
                self.add_entry(code.trim().to_lowercase(), text.trim().to_string());
            } else if let Some((code, text)) = line.split_once(' ') {
                self.add_entry(code.trim().to_lowercase(), text.trim().to_string());
            } else if let Some((code, text)) = line.split_once('\t') {
                self.add_entry(code.trim().to_lowercase(), text.trim().to_string());
            }
        }
    }

    /// 插入一条快捷短语
    pub fn add_entry(&mut self, code: String, text: String) {
        if !code.is_empty() && !text.is_empty() {
            self.phrases.entry(code).or_default().push(text);
        }
    }

    /// 依据输入拼音查询自定义短语
    pub fn lookup(&self, code: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        let key = code.to_lowercase();
        if let Some(list) = self.phrases.get(&key) {
            for text in list {
                candidates.push(
                    AlgoCandidate::new(text, code, 9_999_999)
                        .with_comment("自定义短语")
                        .with_tier(CandidateTier::ExactWord),
                );
            }
        }
        candidates
    }

    /// 获取当前所有短语条目列表 (缩写, 展开文本)
    pub fn all_entries(&self) -> Vec<(String, String)> {
        let mut list = Vec::new();
        for (code, texts) in &self.phrases {
            for text in texts {
                list.push((code.clone(), text.clone()));
            }
        }
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    /// 移除指定缩写和文本的条目
    pub fn remove_entry(&mut self, code: &str, text: &str) -> bool {
        let key = code.to_lowercase();
        if let Some(list) = self.phrases.get_mut(&key) {
            let initial_len = list.len();
            list.retain(|t| t != text);
            let removed = list.len() < initial_len;
            if list.is_empty() {
                self.phrases.remove(&key);
            }
            removed
        } else {
            false
        }
    }

    /// 将当前短语表保存写回文本文件（采用同目录临时文件 + sync_all + rename 原子写入，防并发读取撕裂）
    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        let mut content = String::from(
            "# 微澜输入法自定义短语配置文件\n# 格式：快捷拼音/代码=上屏内容（每行一条）\n",
        );
        let entries = self.all_entries();
        for (code, text) in entries {
            content.push_str(&format!("{}={}\n", code, text));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
        let write_res: std::io::Result<()> = (|| {
            use std::io::Write;
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            fs::rename(&tmp_path, path)?;
            Ok(())
        })();

        if let Err(e) = write_res {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }

        Ok(())
    }
}

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
        crate::custom_phrase_data::DEFAULT_ENGLISH_WORDS
            .iter()
            .enumerate()
            .map(|(idx, w)| (w.to_string(), 100_000_u32.saturating_sub(idx as u32 * 100)))
            .collect()
    }

    /// 获取默认 Unicode 符号与表情映射表
    pub fn default_symbols() -> Vec<(String, String, String)> {
        crate::custom_phrase_data::DEFAULT_SYMBOLS
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
        let temp_path = path_ref.with_extension("aux.tmp");

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

        let mut string_pool = Vec::new();
        let mut string_offset_map: HashMap<String, (u32, u16)> = HashMap::new();

        let mut intern = |s: &str| -> (u32, u16) {
            if let Some(&loc) = string_offset_map.get(s) {
                return loc;
            }
            let off = string_pool.len() as u32;
            let bytes = s.as_bytes();
            let capped_len = bytes.len().min(u16::MAX as usize);
            let len = capped_len as u16;
            string_pool.extend_from_slice(&bytes[..capped_len]);
            string_offset_map.insert(s.to_string(), (off, len));
            (off, len)
        };

        // 1. 写入魔数与头部
        writer
            .write_all(AUX_DICT_MAGIC)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
        writer
            .write_all(&(self.english_words.len() as u32).to_le_bytes())
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
        writer
            .write_all(&(self.symbols.len() as u32).to_le_bytes())
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
        writer
            .write_all(&0u64.to_le_bytes())
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        // 2. 编码英文词库段
        let mut en_buf = Vec::with_capacity(self.english_words.len() * 12);
        for (w, score) in &self.english_words {
            let (off, len) = intern(w);
            en_buf.extend_from_slice(&off.to_le_bytes());
            en_buf.extend_from_slice(&len.to_le_bytes());
            en_buf.extend_from_slice(&0u16.to_le_bytes());
            en_buf.extend_from_slice(&score.to_le_bytes());
        }
        writer
            .write_all(&en_buf)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        // 3. 编码符号表情段
        let mut sym_buf = Vec::with_capacity(self.symbols.len() * 18);
        for (k, sym, desc) in &self.symbols {
            let (k_off, k_len) = intern(k);
            let (s_off, s_len) = intern(sym);
            let (d_off, d_len) = intern(desc);
            sym_buf.extend_from_slice(&k_off.to_le_bytes());
            sym_buf.extend_from_slice(&k_len.to_le_bytes());
            sym_buf.extend_from_slice(&s_off.to_le_bytes());
            sym_buf.extend_from_slice(&s_len.to_le_bytes());
            sym_buf.extend_from_slice(&d_off.to_le_bytes());
            sym_buf.extend_from_slice(&d_len.to_le_bytes());
        }
        writer
            .write_all(&sym_buf)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        // 4. 编码字符串池
        writer
            .write_all(&(string_pool.len() as u32).to_le_bytes())
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
        writer
            .write_all(&string_pool)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
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
        if file_len < 36 {
            return Err(AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: bincode::ErrorKind::Custom("辅助词典文件体积过小或已损坏".to_string())
                    .into(),
            });
        }

        let mut magic = [0u8; 16];
        file.read_exact(&mut magic)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
        if &magic != AUX_DICT_MAGIC {
            return Err(AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: bincode::ErrorKind::Custom("无效的辅助词典魔数标识".to_string()).into(),
            });
        }

        let mut header = [0u8; 16];
        file.read_exact(&mut header)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
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
            return Err(AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: bincode::ErrorKind::Custom(
                    "辅助词典索引段长度超出文件边界，文件可能已损坏".to_string(),
                )
                .into(),
            });
        }

        let mut en_buf = vec![0u8; en_bytes];
        file.read_exact(&mut en_buf)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        let mut sym_buf = vec![0u8; sym_bytes];
        file.read_exact(&mut sym_buf)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        let mut pool_len_buf = [0u8; 4];
        file.read_exact(&mut pool_len_buf)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
        let pool_len = u32::from_le_bytes(pool_len_buf) as usize;
        if meta_end.saturating_add(pool_len) > file_len {
            return Err(AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: bincode::ErrorKind::Custom(
                    "辅助词典字符串池长度超出文件边界，文件可能已损坏".to_string(),
                )
                .into(),
            });
        }

        let mut string_pool = vec![0u8; pool_len];
        file.read_exact(&mut string_pool)
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path_ref),
                source: e,
            })?;

        let get_str = |off: usize, len: usize| -> String {
            if off + len <= string_pool.len() {
                std::str::from_utf8(&string_pool[off..off + len])
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        };

        let mut english_words = Vec::with_capacity(num_english);
        for i in 0..num_english {
            let base = i * 12;
            let off =
                u32::from_le_bytes(en_buf[base..base + 4].try_into().unwrap_or([0; 4])) as usize;
            let len = u16::from_le_bytes(en_buf[base + 4..base + 6].try_into().unwrap_or([0; 2]))
                as usize;
            let score =
                u32::from_le_bytes(en_buf[base + 8..base + 12].try_into().unwrap_or([0; 4]));
            english_words.push((get_str(off, len), score));
        }

        let mut symbols = Vec::with_capacity(num_symbols);
        for i in 0..num_symbols {
            let base = i * 18;
            let k_off =
                u32::from_le_bytes(sym_buf[base..base + 4].try_into().unwrap_or([0; 4])) as usize;
            let k_len = u16::from_le_bytes(sym_buf[base + 4..base + 6].try_into().unwrap_or([0; 2]))
                as usize;
            let s_off =
                u32::from_le_bytes(sym_buf[base + 6..base + 10].try_into().unwrap_or([0; 4]))
                    as usize;
            let s_len =
                u16::from_le_bytes(sym_buf[base + 10..base + 12].try_into().unwrap_or([0; 2]))
                    as usize;
            let d_off =
                u32::from_le_bytes(sym_buf[base + 12..base + 16].try_into().unwrap_or([0; 4]))
                    as usize;
            let d_len =
                u16::from_le_bytes(sym_buf[base + 16..base + 18].try_into().unwrap_or([0; 2]))
                    as usize;
            symbols.push((
                get_str(k_off, k_len),
                get_str(s_off, s_len),
                get_str(d_off, d_len),
            ));
        }

        Ok(Self {
            english_words,
            symbols,
        })
    }
}

pub use crate::custom_phrase_aux::{EnglishSuggestionManager, SymbolSuggestionManager};
