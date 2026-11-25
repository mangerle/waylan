//! 渐进式双数组 Trie 遍历游标与零堆分配音节分支提取器

use super::{DatReader, pinyin_char_to_code};

/// 渐进式双数组 Trie 遍历游标（统一封装节点状态与 Tail 后缀压缩段状态）
///
/// # 设计原理
/// - **实现初衷**：在统一束搜索与 DAT 词组匹配期间，需要按拼音字节/音节逐步推进并在任意位置
///   分支展开简拼或半成品前缀，而无需每次从根节点重新扫描完整字符串。
/// - **核心优势**：零内存分配、支持 `Copy`，且在命中 Tail 压缩段后可 O(1) 完成单音节提取。
/// - **代价与局限**：仅支持合法拼音字节（`a..=z` 与隔音符 `'`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatCursor {
    /// 处于常规 DAT 节点状态
    Trie { node: usize },
    /// 处于 Tail 压缩后缀字节池状态（`offset` 为 Tail 起点，`pos` 为当前已匹配偏移）
    Tail { offset: usize, pos: usize },
}

/// 栈上内联音节分支缓冲（最长 7 字节，零堆分配）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineBranchSyllable {
    bytes: [u8; 8],
    len: u8,
}

impl InlineBranchSyllable {
    /// 从 UTF-8 字节切片构造栈内联音节（超长时返回 `None`）
    #[inline]
    fn from_bytes(src: &[u8]) -> Option<Self> {
        if src.len() > 8 || std::str::from_utf8(src).is_err() {
            return None;
        }
        let mut bytes = [0u8; 8];
        bytes[..src.len()].copy_from_slice(src);
        Some(Self {
            bytes,
            len: src.len() as u8,
        })
    }

    /// 返回音节字符串切片
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: 构造时已校验 `src` 为合法 UTF-8 且 `len <= 8`
        unsafe { std::str::from_utf8_unchecked(&self.bytes[..self.len as usize]) }
    }
}

impl std::ops::Deref for InlineBranchSyllable {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl PartialEq<&str> for InlineBranchSyllable {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<str> for InlineBranchSyllable {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl DatCursor {
    /// 按单个 ASCII 字节在 DAT 中推进游标（支持 `a..=z` 与 `'`）
    #[inline]
    pub fn step_byte(&self, reader: &DatReader<'_>, byte: u8) -> Option<Self> {
        match *self {
            Self::Trie { node } => {
                let code = pinyin_char_to_code(byte);
                if code == 0 || code >= 28 {
                    return None;
                }
                let next = reader.step_to_next_state(node, code)?;
                if next >= reader.base.len() {
                    return None;
                }
                let next_b_val = reader.base[next];
                if next_b_val < 0 {
                    Some(Self::Tail {
                        offset: (-(next_b_val + 1)) as usize,
                        pos: 0,
                    })
                } else {
                    Some(Self::Trie { node: next })
                }
            }
            Self::Tail { offset, pos } => {
                let target = reader.tail.get(offset.checked_add(pos)?)?;
                if *target == byte {
                    Some(Self::Tail {
                        offset,
                        pos: pos + 1,
                    })
                } else {
                    None
                }
            }
        }
    }

    /// 按字符串切片连续推进游标
    #[inline]
    pub fn step_str(&self, reader: &DatReader<'_>, s: &str) -> Option<Self> {
        let mut cursor = *self;
        for &b in s.as_bytes() {
            cursor = cursor.step_byte(reader, b)?;
        }
        Some(cursor)
    }

    /// 若当前游标恰好可作为完整键终止，返回关联的值索引
    #[inline]
    pub fn terminal_value(&self, reader: &DatReader<'_>) -> Option<u32> {
        match *self {
            Self::Trie { node } => reader.match_terminal_leaf(node),
            Self::Tail { offset, pos } => {
                let idx = offset.checked_add(pos)?;
                let slice = reader.tail.get(idx..)?;
                if slice.len() >= 5 && slice[0] == 0 {
                    let val_bytes: [u8; 4] = slice[1..5].try_into().ok()?;
                    Some(u32::from_le_bytes(val_bytes))
                } else {
                    None
                }
            }
        }
    }

    /// 判断当前游标后是否存在终结或 `'` 分隔符（即当前位置是否为完整音节终点）
    #[inline]
    fn is_syllable_boundary(&self, reader: &DatReader<'_>) -> bool {
        self.terminal_value(reader).is_some() || self.step_byte(reader, b'\'').is_some()
    }

    /// 从当前游标出发，收集所有以 `prefix` 开头且在当前词典中实际存在的音节分支
    ///
    /// 返回的每个元素为 `(栈内联音节, 音节末尾且未消费后续分隔符的游标)`，零堆分配。
    pub fn collect_syllable_branches(
        &self,
        reader: &DatReader<'_>,
        prefix: &str,
        max_branches: usize,
        out: &mut Vec<(InlineBranchSyllable, DatCursor)>,
    ) {
        let prefix_bytes = prefix.as_bytes();
        if prefix_bytes.len() > 8 {
            return;
        }
        let Some(after_prefix) = self.step_str(reader, prefix) else {
            return;
        };
        let mut buf = [0u8; 8];
        buf[..prefix_bytes.len()].copy_from_slice(prefix_bytes);
        after_prefix.dfs_syllable_branches(reader, &mut buf, prefix_bytes.len(), max_branches, out);
    }

    /// 在游标当前位置深度优先提取完整音节分支（深度上限 7 字节，防无效长链）
    fn dfs_syllable_branches(
        &self,
        reader: &DatReader<'_>,
        buf: &mut [u8; 8],
        buf_len: usize,
        max_branches: usize,
        out: &mut Vec<(InlineBranchSyllable, DatCursor)>,
    ) {
        if out.len() >= max_branches {
            return;
        }
        match *self {
            Self::Tail { offset, pos } => {
                let Some(slice) = reader.tail.get(offset + pos..) else {
                    return;
                };
                let mut end_rel = 0;
                while end_rel < slice.len() && slice[end_rel] != b'\'' && slice[end_rel] != 0 {
                    if !slice[end_rel].is_ascii_lowercase() || buf_len + end_rel >= 8 {
                        return;
                    }
                    end_rel += 1;
                }
                if buf_len == 0 && end_rel == 0 {
                    return;
                }
                let next_len = buf_len + end_rel;
                buf[buf_len..next_len].copy_from_slice(&slice[..end_rel]);
                if let Some(syl) = InlineBranchSyllable::from_bytes(&buf[..next_len]) {
                    out.push((
                        syl,
                        Self::Tail {
                            offset,
                            pos: pos + end_rel,
                        },
                    ));
                }
            }
            Self::Trie { .. } => {
                if buf_len > 0 && self.is_syllable_boundary(reader) {
                    if let Some(syl) = InlineBranchSyllable::from_bytes(&buf[..buf_len]) {
                        out.push((syl, *self));
                    }
                    if out.len() >= max_branches {
                        return;
                    }
                }
                if buf_len >= 7 {
                    return;
                }
                for b in b'a'..=b'z' {
                    if let Some(next_cursor) = self.step_byte(reader, b) {
                        buf[buf_len] = b;
                        next_cursor.dfs_syllable_branches(
                            reader,
                            buf,
                            buf_len + 1,
                            max_branches,
                            out,
                        );
                        if out.len() >= max_branches {
                            break;
                        }
                    }
                }
            }
        }
    }
}
