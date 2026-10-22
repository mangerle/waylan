use crate::table::LexiconTable;

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

impl LexiconTable {
    /// 创建内置测试词库
    pub fn with_sample_data() -> Self {
        let mut table = Self::new();
        for &(py, text, score) in SAMPLE_LEXICON_ENTRIES {
            table.insert(py, text, score);
        }
        table
    }
}
