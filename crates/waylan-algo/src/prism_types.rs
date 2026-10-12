/// 完整音节切分路径元数据契约
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPath {
    /// 完整音节切分列表（如 ["xi", "an"] 或 ["xian"]）
    pub syllables: Vec<String>,
    /// 末尾未完成键入的剩余前缀（如 "h"）
    pub remaining: String,
    /// 路径代价值（段数越少、匹配越长代价越低，优先排序）
    pub cost: u32,
    /// 是否由容错纠错虚边衍生生成
    pub is_typo: bool,
}

impl SegmentPath {
    /// 转换为带单引号的展示字符串（如 "xi'an"）
    pub fn to_key_string(&self) -> String {
        let mut parts = self.syllables.clone();
        if !self.remaining.is_empty() {
            parts.push(self.remaining.clone());
        }
        parts.join("'")
    }
}

/// 现代汉语普通话标准声母列表（双字母翘舌声母排在最前优先匹配）
pub static STANDARD_INITIALS: &[&str] = &[
    "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r",
    "z", "c", "s", "y", "w",
];
