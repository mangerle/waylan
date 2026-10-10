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

    /// 将当前短语表保存写回文本文件
    pub fn save_to_file(&self, path: &Path) -> std::io::Result<()> {
        let mut content = String::from("# 微澜输入法自定义短语配置文件\n# 格式：快捷拼音/代码=上屏内容（每行一条）\n");
        let entries = self.all_entries();
        for (code, text) in entries {
            content.push_str(&format!("{}={}\n", code, text));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)
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
        let base_words = [
            "action", "admin", "android", "api", "apple", "app", "application", "async",
            "base", "boolean", "branch", "build", "byte",
            "cancel", "cargo", "channel", "checkout", "class", "client", "clone", "cloud", "code", "commit", "config", "const",
            "daemon", "database", "date", "debug", "delete", "diff", "docker", "documentation",
            "egui", "engine", "error", "event",
            "false", "file", "float", "format", "function",
            "git", "github", "gitlab", "google", "graph",
            "hash", "header", "hello", "home", "html", "http", "https",
            "icon", "import", "index", "init", "input", "insert", "install", "integer", "interface", "issue",
            "json",
            "key", "keyboard",
            "layer", "layout", "license", "linux", "list", "log", "login",
            "main", "manager", "master", "match", "menu", "message", "method", "model", "module",
            "name", "native", "network", "node", "null", "number",
            "object", "open", "option", "output",
            "package", "page", "panel", "parser", "patch", "path", "pinyin", "plugin", "popup", "preedit", "print", "project", "protocol", "pull", "push",
            "query", "queue",
            "readme", "record", "release", "remote", "remove", "render", "repository", "request", "reset", "resource", "response", "result", "return", "rust",
            "save", "scan", "schema", "scope", "screen", "script", "search", "select", "server", "service", "session", "setting", "settings", "shell", "signal", "size", "socket", "source", "start", "state", "status", "stop", "string", "struct", "style", "surface", "switch", "system",
            "table", "task", "terminal", "test", "text", "theme", "time", "timer", "title", "token", "toml", "total", "trace", "tray", "true", "type",
            "ubuntu", "unicode", "unit", "update", "upload", "url", "user", "util",
            "value", "variable", "vector", "version", "view",
            "wait", "warning", "waylan", "wayland", "web", "while", "window", "workspace", "write",
            "yaml",
        ];
        base_words
            .iter()
            .enumerate()
            .map(|(idx, w)| (w.to_string(), 100_000_u32.saturating_sub(idx as u32 * 100)))
            .collect()
    }

    /// 获取默认 Unicode 符号与表情映射表
    pub fn default_symbols() -> Vec<(String, String, String)> {
        vec![
            ("dianzan".to_string(), "\u{1F44D}".to_string(), "强/点赞".to_string()),
            ("daxiao".to_string(), "\u{1F602}".to_string(), "大笑".to_string()),
            ("weixiao".to_string(), "\u{1F60A}".to_string(), "微笑".to_string()),
            ("shengri".to_string(), "\u{1F382}".to_string(), "生日蛋糕".to_string()),
            ("hongxin".to_string(), "\u{2764}\u{FE0F}".to_string(), "红心".to_string()),
            ("aixin".to_string(), "\u{1F495}".to_string(), "爱心".to_string()),
            ("guzhang".to_string(), "\u{1F44F}".to_string(), "鼓掌".to_string()),
            ("jiayou".to_string(), "\u{1F4AA}".to_string(), "加油/肌肉".to_string()),
            ("xianhua".to_string(), "\u{1F339}".to_string(), "鲜花/玫瑰".to_string()),
            ("taiyang".to_string(), "\u{2600}\u{FE0F}".to_string(), "太阳".to_string()),
            ("yueliang".to_string(), "\u{1F319}".to_string(), "月亮".to_string()),
            ("shengli".to_string(), "\u{270C}\u{FE0F}".to_string(), "胜利/剪刀手".to_string()),
            ("woshou".to_string(), "\u{1F91D}".to_string(), "握手".to_string()),
            ("fennu".to_string(), "\u{1F621}".to_string(), "愤怒".to_string()),
            ("kuqi".to_string(), "\u{1F62D}".to_string(), "哭泣".to_string()),
            ("sikao".to_string(), "\u{1F914}".to_string(), "思考".to_string()),
            ("baiye".to_string(), "\u{1F31F}".to_string(), "星星".to_string()),
            ("huoyan".to_string(), "\u{1F525}".to_string(), "火焰".to_string()),
            ("zhadan".to_string(), "\u{1F4A3}".to_string(), "炸弹".to_string()),
            ("liwu".to_string(), "\u{1F381}".to_string(), "礼物".to_string()),
            ("jia".to_string(), "±".to_string(), "正负号".to_string()),
            ("jifen".to_string(), "∫".to_string(), "积分".to_string()),
            ("alpha".to_string(), "α".to_string(), "希腊字母".to_string()),
            ("beta".to_string(), "β".to_string(), "希腊字母".to_string()),
        ]
    }

    /// 持久化保存为紧凑二进制词库文件 (WAYLAN_AUXCOMP01)
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
        let mut string_offset_map: HashMap<String, (u32, u16)> = HashMap::new();

        let mut intern = |s: &str| -> (u32, u16) {
            if let Some(&loc) = string_offset_map.get(s) {
                return loc;
            }
            let off = string_pool.len() as u32;
            let bytes = s.as_bytes();
            let len = bytes.len() as u16;
            string_pool.extend_from_slice(bytes);
            string_offset_map.insert(s.to_string(), (off, len));
            (off, len)
        };

        // 1. 写入魔数与头部
        writer.write_all(AUX_DICT_MAGIC).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        writer.write_all(&(self.english_words.len() as u32).to_le_bytes()).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;
        writer.write_all(&(self.symbols.len() as u32).to_le_bytes()).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;
        writer.write_all(&0u64.to_le_bytes()).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;

        // 2. 编码英文词库段
        let mut en_buf = Vec::with_capacity(self.english_words.len() * 12);
        for (w, score) in &self.english_words {
            let (off, len) = intern(w);
            en_buf.extend_from_slice(&off.to_le_bytes());
            en_buf.extend_from_slice(&len.to_le_bytes());
            en_buf.extend_from_slice(&0u16.to_le_bytes());
            en_buf.extend_from_slice(&score.to_le_bytes());
        }
        writer.write_all(&en_buf).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;

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
        writer.write_all(&sym_buf).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;

        // 4. 编码字符串池
        writer.write_all(&(string_pool.len() as u32).to_le_bytes()).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;
        writer.write_all(&string_pool).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;
        writer.flush().map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;
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
        if metadata.len() < 32 {
            return Err(AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: bincode::ErrorKind::Custom("辅助词典文件体积过小或已损坏".to_string()).into(),
            });
        }

        let mut magic = [0u8; 16];
        file.read_exact(&mut magic).map_err(|e| AlgoError::DictionaryIo {
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
        file.read_exact(&mut header).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let num_english = u32::from_le_bytes(header[0..4].try_into().unwrap_or([0; 4])) as usize;
        let num_symbols = u32::from_le_bytes(header[4..8].try_into().unwrap_or([0; 4])) as usize;

        let en_bytes = num_english * 12;
        let mut en_buf = vec![0u8; en_bytes];
        file.read_exact(&mut en_buf).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;

        let sym_bytes = num_symbols * 18;
        let mut sym_buf = vec![0u8; sym_bytes];
        file.read_exact(&mut sym_buf).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;

        let mut pool_len_buf = [0u8; 4];
        file.read_exact(&mut pool_len_buf).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;
        let pool_len = u32::from_le_bytes(pool_len_buf) as usize;

        let mut string_pool = vec![0u8; pool_len];
        file.read_exact(&mut string_pool).map_err(|e| AlgoError::DictionaryIo { path: PathBuf::from(path_ref), source: e })?;

        let get_str = |off: usize, len: usize| -> String {
            if off + len <= string_pool.len() {
                std::str::from_utf8(&string_pool[off..off + len]).unwrap_or("").to_string()
            } else {
                String::new()
            }
        };

        let mut english_words = Vec::with_capacity(num_english);
        for i in 0..num_english {
            let base = i * 12;
            let off = u32::from_le_bytes(en_buf[base..base + 4].try_into().unwrap_or([0; 4])) as usize;
            let len = u16::from_le_bytes(en_buf[base + 4..base + 6].try_into().unwrap_or([0; 2])) as usize;
            let score = u32::from_le_bytes(en_buf[base + 8..base + 12].try_into().unwrap_or([0; 4]));
            english_words.push((get_str(off, len), score));
        }

        let mut symbols = Vec::with_capacity(num_symbols);
        for i in 0..num_symbols {
            let base = i * 18;
            let k_off = u32::from_le_bytes(sym_buf[base..base + 4].try_into().unwrap_or([0; 4])) as usize;
            let k_len = u16::from_le_bytes(sym_buf[base + 4..base + 6].try_into().unwrap_or([0; 2])) as usize;
            let s_off = u32::from_le_bytes(sym_buf[base + 6..base + 10].try_into().unwrap_or([0; 4])) as usize;
            let s_len = u16::from_le_bytes(sym_buf[base + 10..base + 12].try_into().unwrap_or([0; 2])) as usize;
            let d_off = u32::from_le_bytes(sym_buf[base + 12..base + 16].try_into().unwrap_or([0; 4])) as usize;
            let d_len = u16::from_le_bytes(sym_buf[base + 16..base + 18].try_into().unwrap_or([0; 2])) as usize;
            symbols.push((get_str(k_off, k_len), get_str(s_off, s_len), get_str(d_off, d_len)));
        }

        Ok(Self {
            english_words,
            symbols,
        })
    }
}

/// 常用高频英文单词联想管理器
#[derive(Debug, Clone)]
pub struct EnglishSuggestionManager {
    /// 单词与词频列表（按小写字母序排序，便于二分前缀查找）
    words: Vec<(String, u32)>,
    /// 精确匹配哈希索引 (小写单词 -> (原词, 词频))
    exact_index: HashMap<String, (String, u32)>,
}

impl Default for EnglishSuggestionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl EnglishSuggestionManager {
    /// 创建带默认保底词汇的管理器
    pub fn new() -> Self {
        Self::from_words(AuxiliaryDict::default_english_words())
    }

    /// 从单词列表加载
    pub fn from_words(words: Vec<(String, u32)>) -> Self {
        let mut exact_index = HashMap::with_capacity(words.len());
        for (w, score) in &words {
            exact_index.insert(w.to_lowercase(), (w.clone(), *score));
        }
        let mut sorted_words = words;
        sorted_words.sort_by_key(|a| a.0.to_lowercase());
        Self {
            words: sorted_words,
            exact_index,
        }
    }

    /// 获取当前词目总数
    pub fn len(&self) -> usize {
        self.exact_index.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.exact_index.is_empty()
    }

    /// 匹配输入前缀对应的英文单词
    pub fn lookup(&self, input: &str) -> Vec<AlgoCandidate> {
        let input_lower = input.to_lowercase();
        let mut results = Vec::new();

        // 1. 完全精确匹配 (O(1) 哈希命中)
        if let Some((orig_word, score)) = self.exact_index.get(&input_lower) {
            results.push(
                AlgoCandidate::new(orig_word, input, 4_500_000_u32.saturating_add(*score))
                    .with_comment("英文单词")
                    .with_tier(CandidateTier::Acronym),
            );
            return results;
        }

        // 2. 前缀匹配（输入至少 3 个字母时提供补全联想，利用二分查找定位）
        if input_lower.len() >= 3 {
            let idx = match self.words.binary_search_by(|(w, _)| {
                w.to_lowercase().cmp(&input_lower)
            }) {
                Ok(i) => i,
                Err(i) => i,
            };

            for (w, score) in &self.words[idx..] {
                let w_lower = w.to_lowercase();
                if w_lower.starts_with(&input_lower) {
                    if w_lower != input_lower {
                        results.push(
                            AlgoCandidate::new(w, input, 3_000_000_u32.saturating_add(*score / 2))
                                .with_comment("英文联想")
                                .with_tier(CandidateTier::PrefixPartial),
                        );
                        if results.len() >= 3 {
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        }

        results
    }
}

/// 常用表情与特殊符号联想管理器（严格使用 Unicode 码点，代码内无 Emoji 字符）
#[derive(Debug, Clone)]
pub struct SymbolSuggestionManager {
    /// 拼音键到 (符号文本, 描述) 的哈希索引映射
    index: HashMap<String, Vec<(String, String)>>,
    /// 符号条目总数
    total_len: usize,
}

impl Default for SymbolSuggestionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolSuggestionManager {
    /// 创建带默认保底符号表的管理器
    pub fn new() -> Self {
        Self::from_symbols(AuxiliaryDict::default_symbols())
    }

    /// 从符号列表加载构建哈希倒排索引
    pub fn from_symbols(symbols: Vec<(String, String, String)>) -> Self {
        let total_len = symbols.len();
        let mut index: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (py, sym, desc) in symbols {
            index.entry(py).or_default().push((sym, desc));
        }
        Self { index, total_len }
    }

    /// 获取当前符号总数
    pub fn len(&self) -> usize {
        self.total_len
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.total_len == 0
    }

    /// 检索符号表情候选 (O(1) 哈希查询)
    pub fn lookup(&self, pinyin: &str) -> Vec<AlgoCandidate> {
        let pinyin_clean = pinyin.replace('\'', "");
        let mut results = Vec::new();
        if let Some(list) = self.index.get(&pinyin_clean) {
            for (sym, desc) in list {
                results.push(
                    AlgoCandidate::new(sym, pinyin, 4_800_000)
                        .with_comment(desc)
                        .with_tier(CandidateTier::Acronym),
                );
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_phrase_parse_and_lookup() {
        let mut manager = CustomPhraseManager::new();
        manager.parse_content("mymail=test@test.com\naddr 北京市海淀区");
        let mail_res = manager.lookup("mymail");
        assert_eq!(mail_res.len(), 1);
        assert_eq!(mail_res[0].text, "test@test.com");

        let addr_res = manager.lookup("addr");
        assert_eq!(addr_res.len(), 1);
        assert_eq!(addr_res[0].text, "北京市海淀区");
    }

    #[test]
    fn test_custom_phrase_all_entries_remove_save() {
        let mut manager = CustomPhraseManager::new();
        manager.add_entry("mymail".to_string(), "a@b.com".to_string());
        manager.add_entry("addr".to_string(), "北京".to_string());

        let entries = manager.all_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ("addr".to_string(), "北京".to_string()));

        assert!(manager.remove_entry("addr", "北京"));
        assert_eq!(manager.all_entries().len(), 1);
        assert!(!manager.remove_entry("non_exist", "none"));
    }

    #[test]
    fn test_english_suggestions() {
        let eng = EnglishSuggestionManager::new();
        let exact = eng.lookup("rust");
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].text, "rust");

        let prefix = eng.lookup("way");
        assert!(!prefix.is_empty());
        assert!(prefix.iter().any(|c| c.text == "wayland" || c.text == "waylan"));
    }

    #[test]
    fn test_symbol_suggestions() {
        let sym = SymbolSuggestionManager::new();
        let res = sym.lookup("dianzan");
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].text, "\u{1F44D}");
    }

    #[test]
    fn test_auxiliary_dict_save_and_load() {
        let dict = AuxiliaryDict::with_default_data();
        assert!(!dict.english_words.is_empty());
        assert!(!dict.symbols.is_empty());

        let temp_path = std::env::temp_dir().join("test_aux_dict.bin");
        dict.save_to_file(&temp_path).expect("save aux dict");
        let loaded = AuxiliaryDict::load_from_file(&temp_path).expect("load aux dict");
        assert_eq!(dict, loaded);
        let _ = std::fs::remove_file(temp_path);
    }
}
