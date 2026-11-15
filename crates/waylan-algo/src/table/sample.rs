use crate::compact_writer::compile_entries_to_compact_bytes;
use crate::table::{CompactMmapLexicon, LexiconTable};
use std::collections::HashMap;
use std::sync::OnceLock;

/// 测试与自检用的最小样例词表（拼音键、词条文本、原始词频三元组）
pub const SAMPLE_LEXICON_ENTRIES: &[(&str, &str, u32)] = &[
    ("ni'hao", "你好", 10_000),
    ("ni", "你", 9_500),
    ("hao", "好", 9_200),
    ("xian", "先", 7_200),
    ("xian", "现", 7_100),
    ("xian", "显", 6_800),
    ("xi'an", "西安", 7_500),
    ("xi", "西", 8_000),
    ("an", "安", 8_200),
    ("wo", "我", 9_900),
    ("wo'men", "我们", 11_000),
    ("zhong'guo", "中国", 12_000),
    ("zhong", "中", 8_000),
    ("guo", "国", 8_500),
    ("ri'qi", "日期", 10_000),
    ("shi'jian", "时间", 10_000),
    ("bei'jing", "北京", 15_000),
    ("yong'yuan'de'shen", "永远的神", 20_000),
    ("en", "恩", 8_500),
    ("en", "摁", 5_000),
    ("n", "嗯", 200_000),
    ("ng", "嗯", 200_000),
    ("ng'ng", "嗯嗯", 12_000),
    ("ng'hao", "嗯好", 11_000),
    ("me", "么", 8_000),
    ("m", "呣", 200_000),
];

/// 缓存内置样例词库编译后的紧凑二进制字节流，避免重复建树开销
static SAMPLE_COMPACT_BYTES: OnceLock<Vec<u8>> = OnceLock::new();

/// 构建样例词条原始映射表
fn build_sample_entries_map() -> HashMap<String, Vec<(String, u32)>> {
    let mut entries: HashMap<String, Vec<(String, u32)>> =
        HashMap::with_capacity(SAMPLE_LEXICON_ENTRIES.len());
    for &(py, text, score) in SAMPLE_LEXICON_ENTRIES {
        let list = entries.entry(py.to_string()).or_default();
        LexiconTable::upsert_entry(list, text.to_string(), score);
    }
    entries
}

impl LexiconTable {
    /// 创建内置测试词库（直接基于内存编译的紧凑 DAT 视图运行）
    pub fn with_sample_data() -> Self {
        let dynamic_entries = build_sample_entries_map();
        let bytes =
            SAMPLE_COMPACT_BYTES.get_or_init(|| compile_entries_to_compact_bytes(&dynamic_entries));
        let compact_mmap = CompactMmapLexicon::from_bytes(bytes).ok();
        Self {
            dynamic_entries,
            compact_mmap,
            compact_bigram: None,
        }
    }
}
