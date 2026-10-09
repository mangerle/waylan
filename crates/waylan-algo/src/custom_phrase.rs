use crate::error::AlgoError;
use crate::types::{AlgoCandidate, CandidateTier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
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

    /// 持久化保存为二进制文件 (bincode)
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), AlgoError> {
        let path_ref = path.as_ref();
        if let Some(parent) = path_ref.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = fs::File::create(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let writer = std::io::BufWriter::new(file);
        bincode::serialize_into(writer, self).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: std::io::Error::other(e.to_string()),
        })?;
        Ok(())
    }

    /// 从二进制文件加载
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let file = fs::File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let reader = std::io::BufReader::new(file);
        let dict: Self = bincode::deserialize_from(reader).map_err(|e| AlgoError::DeserializeFailed {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        Ok(dict)
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
        sorted_words.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
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
