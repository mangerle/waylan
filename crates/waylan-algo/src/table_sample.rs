use crate::table::LexiconTable;

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
