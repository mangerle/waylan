//! 用户自定义短语管理器模块
//!
//! 支持用户在纯文本文件 (~/.config/waylan/phrases.txt) 中维护个性化快捷短语。
//! 格式支持 "缩写=展开短语" 或 "缩写 展开短语"（如 mymail=abc@domain.com）。

use super::dynamic_macro::expand_dynamic_macros;
use crate::types::{AlgoCandidate, CandidateTier};
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
    /// 创建空的自定义短语管理器
    pub fn new() -> Self {
        Self {
            phrases: HashMap::new(),
        }
    }

    /// 获取默认自定义短语配置文件路径
    pub fn default_path() -> PathBuf {
        waylan_core::config::user_config_dir()
            .map(|dir| dir.join("phrases.txt"))
            .unwrap_or_else(|| PathBuf::from("phrases.txt"))
    }

    /// 从文件加载自定义短语，若不存在可自动创建模板
    ///
    /// 体积超出安全上限或读取失败时降级为空短语表并记录告警，
    /// 保证输入法仍可正常打字，不因单个用户数据文件异常而中断会话。
    pub fn load_or_init(path: &Path) -> Self {
        let mut manager = Self::new();
        if path.exists() {
            if let Err(e) =
                crate::fs_guard::ensure_within_limit(path, waylan_core::MAX_PHRASE_FILE_BYTES)
            {
                log::warn!("[算法引擎] 跳过加载异常体积的自定义短语文件: {}", e);
                return manager;
            }
            match fs::read_to_string(path) {
                Ok(content) => manager.parse_content(&content),
                Err(e) => {
                    log::warn!(
                        "[算法引擎] 读取自定义短语文件失败，按空短语表继续，路径: {:?}，原因: {}",
                        path,
                        e
                    );
                }
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

    /// 解析文本内容（支持 \\n、\\r、\\t 转义）
    pub fn parse_content(&mut self, content: &str) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((code, text)) = line.split_once('=') {
                self.add_entry(code.trim().to_lowercase(), Self::unescape_text(text.trim()));
            } else if let Some((code, text)) = line.split_once(' ') {
                self.add_entry(code.trim().to_lowercase(), Self::unescape_text(text.trim()));
            } else if let Some((code, text)) = line.split_once('\t') {
                self.add_entry(code.trim().to_lowercase(), Self::unescape_text(text.trim()));
            }
        }
    }

    /// 对配置文本中的 \\n、\\r、\\t 转义字符进行解码
    pub(crate) fn unescape_text(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// 将多行文本转义为单行存储格式
    pub(crate) fn escape_text(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\\' => out.push_str("\\\\"),
                _ => out.push(c),
            }
        }
        out
    }

    /// 插入一条快捷短语
    pub fn add_entry(&mut self, code: String, text: String) {
        if !code.is_empty() && !text.is_empty() {
            self.phrases.entry(code).or_default().push(text);
        }
    }

    /// 依据输入拼音查询自定义短语（实时求值并展开其中的动态宏指令）
    pub fn lookup(&self, code: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        let key = code.to_lowercase();
        if let Some(list) = self.phrases.get(&key) {
            for text in list {
                let expanded = expand_dynamic_macros(text);
                candidates.push(
                    AlgoCandidate::new(expanded, code, 9_999_999)
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
            content.push_str(&format!("{}={}\n", code, Self::escape_text(&text)));
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
