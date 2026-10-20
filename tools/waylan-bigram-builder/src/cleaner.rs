//! 语料清洗、中文分句与二元词对切分提取器
//!
//! # 设计原理
//! - **实现初衷**：从原始语料（JSONL、SFT 对话、普通文本）中剥离代码块与非中文字符，
//!   基于正规标点分句后调用 Jieba 提取连续二元词对。
//! - **核心优势**：纯文本切分与清洗逻辑高度内聚，零磁盘 IO 依赖。

use jieba_rs::Jieba;
use regex::Regex;
use serde_json::Value;

/// 构建预处理与分词清洗器
pub struct Cleaner {
    re_code: Regex,
    re_split: Regex,
    re_chinese: Regex,
}

/// 语料词对收集参数
///
/// 承载待清洗的原始语料、词对权重、分词器实例与词对输出容器。
/// 采用参数对象聚合可避免长参数列表在调用处发生参数错位；输出容器以可变引用
/// 形式随结构体传入，保证词对在调用方容器中原地累加。
pub struct TextPairsParams<'a> {
    /// 待清洗的原始语料文本
    pub text: &'a str,
    /// 词对权重（用户轮次为 2，助手轮次为 1）
    pub weight: u32,
    /// 中文分词器实例
    pub jieba: &'a Jieba,
    /// 词对输出容器（追加写入）
    pub out: &'a mut Vec<(String, String, u32)>,
}

impl Cleaner {
    /// 初始化清洗器正则表达式
    pub fn new() -> Self {
        Self {
            re_code: Regex::new(r"```[\s\S]*?```").expect("代码块正则语法合法"),
            re_split: Regex::new(r"[，。！？；、\n\r\t]+").expect("分句标点正则语法合法"),
            re_chinese: Regex::new(r"[\u4e00-\u9fff]").expect("汉字区间正则语法合法"),
        }
    }

    /// 清洗长文本并切分为高置信度独立中文句子
    pub fn clean_and_split(&self, text: &str) -> Vec<String> {
        let text_no_code = self.re_code.replace_all(text, "");
        self.re_split
            .split(&text_no_code)
            .map(str::trim)
            .filter(|trimmed| {
                let char_count = trimmed.chars().count();
                (2..=80).contains(&char_count)
                    && (self.re_chinese.find_iter(trimmed).count() as f64 / char_count as f64)
                        >= 0.6
            })
            .map(str::to_string)
            .collect()
    }

    /// 从单段文本中切分提取相邻二元词对并累加至输出容器
    pub fn collect_text_pairs(&self, params: TextPairsParams<'_>) {
        let TextPairsParams {
            text,
            weight,
            jieba,
            out,
        } = params;
        for sent in self.clean_and_split(text) {
            let words = jieba.cut(&sent, false);
            for pair in words.windows(2) {
                out.push((pair[0].to_string(), pair[1].to_string(), weight));
            }
        }
    }

    /// 解析单行 JSONL 语料并提取相邻二元词对
    pub fn extract_line_pairs(
        &self,
        raw_line: &str,
        is_sft: bool,
        jieba: &Jieba,
    ) -> Vec<(String, String, u32)> {
        let mut local_res = Vec::new();
        let Ok(val) = serde_json::from_str::<Value>(raw_line) else {
            return local_res;
        };
        if is_sft {
            if let Some(convs) = val.get("conversations").and_then(|v| v.as_array()) {
                for turn in convs {
                    let content = turn.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let role = turn.get("role").and_then(|v| v.as_str()).unwrap_or("");
                    let weight: u32 = if role == "user" { 2 } else { 1 };
                    self.collect_text_pairs(TextPairsParams {
                        text: content,
                        weight,
                        jieba,
                        out: &mut local_res,
                    });
                }
            }
        } else if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
            self.collect_text_pairs(TextPairsParams {
                text,
                weight: 1,
                jieba,
                out: &mut local_res,
            });
        }
        local_res
    }
}
