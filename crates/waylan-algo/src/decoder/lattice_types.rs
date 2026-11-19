//! 拼音音节格数据结构与边类型定义

use crate::types::FuzzyConfig;
use waylan_core::config::TypoCorrectionRules;

/// 容错边细分类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypoKind {
    /// 邻键误触替换
    Neighbor,
    /// 相邻字母倒置（含音节内与跨音节倒置）
    Transpose,
    /// 漏按字母（含边界共享尾字母漏按，如 zhonguo -> zhong'guo）
    Missing,
    /// 多按字母（如 shuang -> shuaang）
    Extra,
    /// 规则驱动的软纠错（如韵母/声母习惯纠错）
    RuleTypo,
}

/// 音节格边的语言学类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// 标准全拼精确音节
    Exact,
    /// 模糊音扩展音节
    Fuzzy,
    /// 习惯音/叹词规范化映射（如 en -> n/ng）
    RuleNormalized,
    /// 四类容错纠错边
    Typo(TypoKind),
    /// 混简拼单声母边（用于任意位置的全拼混简拼展开）
    Initial,
    /// 仅位于输入末尾的残余半成品前缀边
    PartialTail,
}

/// 栈内定长单音节存储（标准拼音音节与容错音节最长不超过 7 字节，零堆分配）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InlineSyllable {
    bytes: [u8; 8],
    len: u8,
}

impl InlineSyllable {
    /// 从字符串切片构造栈内定长音节
    #[inline]
    pub fn new(s: &str) -> Self {
        let raw = s.as_bytes();
        let copy_len = raw.len().min(8);
        let mut bytes = [0u8; 8];
        bytes[..copy_len].copy_from_slice(&raw[..copy_len]);
        Self {
            bytes,
            len: copy_len as u8,
        }
    }

    /// 以 `&str` 视图访问音节文本
    #[inline]
    pub fn as_str(&self) -> &str {
        // Safety: 构造时仅从合法 UTF-8/ASCII 音节切片拷贝而来
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
}

/// 单条音节格边携带的音节序列（栈内定长 `Copy`，最多 2 个音节，消除堆分配）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdgeSyllables {
    items: [InlineSyllable; 2],
    len: u8,
}

impl EdgeSyllables {
    /// 构造仅包含 1 个音节的边音节序列
    #[inline]
    pub fn from_one(s: &str) -> Self {
        let empty = InlineSyllable {
            bytes: [0u8; 8],
            len: 0,
        };
        Self {
            items: [InlineSyllable::new(s), empty],
            len: 1,
        }
    }

    /// 构造包含 2 个音节的复合边音节序列
    #[inline]
    pub fn from_two(s1: &str, s2: &str) -> Self {
        Self {
            items: [InlineSyllable::new(s1), InlineSyllable::new(s2)],
            len: 2,
        }
    }

    /// 返回包含的音节数量（1 或 2）
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// 判断是否为空
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 获取首音节切片
    #[inline]
    pub fn first(&self) -> Option<&str> {
        (self.len > 0).then(|| self.items[0].as_str())
    }

    /// 按序迭代携带的音节文本切片
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.items[..self.len as usize]
            .iter()
            .map(InlineSyllable::as_str)
    }
}

impl<const N: usize> PartialEq<[&str; N]> for EdgeSyllables {
    fn eq(&self, other: &[&str; N]) -> bool {
        self.len as usize == N && self.iter().zip(other.iter().copied()).all(|(a, b)| a == b)
    }
}

impl PartialEq<Vec<&str>> for EdgeSyllables {
    fn eq(&self, other: &Vec<&str>) -> bool {
        self.len as usize == other.len()
            && self.iter().zip(other.iter().copied()).all(|(a, b)| a == b)
    }
}

/// 音节格上的单条局部有向边
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyllableEdge {
    /// 原始输入中的起始字节偏移
    pub start: usize,
    /// 原始输入中的结束字节偏移
    pub end: usize,
    /// 该边对应的目标标准音节序列（常规为 1 个，跨音节倒置或边界共享漏按为 2 个）
    pub syllables: EdgeSyllables,
    /// 边类别
    pub kind: EdgeKind,
    /// 本边消耗的纠错次数（正常/模糊/简拼/半成品为 0，单处容错为 1）
    pub typo_count: u8,
    /// 图边基础惩罚代价（越小越优）
    pub cost: u32,
}

/// 音节格构建配置选项
#[derive(Debug, Clone, Default)]
pub struct LatticeOptions {
    /// 模糊音规则配置
    pub fuzzy: FuzzyConfig,
    /// 纠错规则配置
    pub typo_rules: TypoCorrectionRules,
    /// 是否生成单字母混简拼边
    pub enable_mixed_jianpin: bool,
    /// 是否生成单音节/跨音节容错边
    pub enable_typo_edges: bool,
}
