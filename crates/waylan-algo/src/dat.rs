/// 拼音字符编码映射函数
///
/// 字符集映射规格：
/// - `\0` -> 0 (终结符，用于叶子状态匹配)
/// - `'`  -> 1 (拼音音节隔音符，如 "xi'an")
/// - `a`..=`z` -> 2..=27
/// - 其他非法字符 -> 28
#[inline]
pub fn pinyin_char_to_code(b: u8) -> usize {
    match b {
        0 => 0,
        b'\'' => 1,
        b'a'..=b'z' => 2 + (b - b'a') as usize,
        _ => 28,
    }
}

/// 双数组 Trie (Double-Array Trie, DAT) 紧凑构建器与持有者 (集成 Tail 后缀压缩)
///
/// # 算法原理
/// 采用 Aoe (1989) 经典双数组自动机架构 + 后缀 Tail 压缩：
/// - 对具有多个分支的公共前缀，在 `base` 与 `check` 数组中建立状态转移；
/// - 对无分支的单链长尾（如长拼音后缀），将其剩余字符直接截断并存入紧凑字节池 `tail` 中；
/// - 使得 DAT 节点总数从数百万骤降，将内存与文件体积缩减 70% 以上，同时查询速度保持纳秒级。
#[derive(Debug, Clone, Default)]
pub struct DoubleArrayTrie {
    /// 状态基准偏移数组
    pub base: Vec<i32>,
    /// 状态父节点归属校验数组
    pub check: Vec<u32>,
    /// 单链长尾字符池 (Tail 压缩段)
    pub tail: Vec<u8>,
}

impl DoubleArrayTrie {
    /// 从已按字典序排序且去重的拼音键及其关联值索引构建带 Tail 压缩的 DAT
    ///
    /// 构建逻辑委托给 [`crate::dat_builder::build_dat`]。
    pub fn build(sorted_keys: &[String], values: &[u32]) -> Self {
        crate::dat_builder::build_dat(sorted_keys, values)
    }

    /// 返回空状态的 DAT
    #[must_use]
    pub fn empty() -> Self {
        Self {
            base: vec![0],
            check: vec![0],
            tail: Vec::new(),
        }
    }
}

/// 零拷贝只读双数组 Trie 视图 (基于 mmap 内存切片，支持 Tail 后缀压缩)
#[derive(Debug, Clone, Copy)]
pub struct DatReader<'a> {
    base: &'a [i32],
    check: &'a [u32],
    tail: &'a [u8],
}

impl<'a> DatReader<'a> {
    /// 从只读切片构建零拷贝 DAT 视图
    pub fn new(base: &'a [i32], check: &'a [u32], tail: &'a [u8]) -> Self {
        Self { base, check, tail }
    }

    /// 精确匹配拼音键，成功则返回绑定的关联值
    #[inline]
    pub fn exact_match(&self, key: &str) -> Option<u32> {
        let key_bytes = key.as_bytes();
        let mut curr = 1usize;
        let mut depth = 0usize;

        while depth < key_bytes.len() {
            let code = pinyin_char_to_code(key_bytes[depth]);
            if code == 0 || code >= 28 {
                return None; // 拦截空字节穿透与非合法拼音字符
            }

            let next = self.step_to_next_state(curr, code)?;

            // 检查新状态是否命中 Tail 压缩
            let next_b_val = self.base[next];
            if next_b_val < 0 {
                return self.match_compressed_tail(&key_bytes[depth + 1..], next_b_val);
            }

            curr = next;
            depth += 1;
        }

        // 走终止符 code 0
        self.match_terminal_leaf(curr)
    }

    /// 获取指向 DAT 根节点的渐进游标
    #[inline]
    pub fn root_cursor(&self) -> DatCursor {
        DatCursor::Trie { node: 1 }
    }

    /// 在 DAT 中执行单字符状态转移，命中非法分支时返回 None
    #[inline]
    fn step_to_next_state(&self, curr: usize, code: usize) -> Option<usize> {
        if curr >= self.base.len() {
            return None;
        }
        let b_val = self.base[curr];
        if b_val <= 0 {
            return None;
        }

        let next = (b_val as usize) + code;
        if next >= self.check.len() || self.check[next] != curr as u32 {
            return None;
        }
        Some(next)
    }

    /// 匹配 DAT 终止符分支并取出叶子节点上绑定的关联值
    #[inline]
    fn match_terminal_leaf(&self, curr: usize) -> Option<u32> {
        if curr >= self.base.len() {
            return None;
        }
        let b_val = self.base[curr];
        if b_val <= 0 {
            return None;
        }

        let leaf = b_val as usize;
        if leaf >= self.check.len() || self.check[leaf] != curr as u32 {
            return None;
        }

        let val = self.base[leaf];
        if val < 0 {
            Some((-(val + 1)) as u32)
        } else {
            None
        }
    }

    /// 在命中 Tail 压缩分支时匹配剩余字符并取出关联值
    fn match_compressed_tail(&self, remaining: &[u8], next_b_val: i32) -> Option<u32> {
        let tail_offset = (-(next_b_val + 1)) as usize;
        if tail_offset >= self.tail.len() {
            return None;
        }
        if remaining.contains(&0) {
            return None;
        }
        let tail_slice = &self.tail[tail_offset..];

        // 检查 remaining 是否与 tail_slice 完全一致且刚好以 0 结束
        let rem_len = remaining.len();
        if tail_slice.len() < rem_len + 5 {
            return None;
        }
        if &tail_slice[..rem_len] != remaining || tail_slice[rem_len] != 0 {
            return None;
        }

        // 取出 4 字节关联值
        let val_bytes: [u8; 4] = tail_slice[rem_len + 1..rem_len + 5].try_into().ok()?;
        Some(u32::from_le_bytes(val_bytes))
    }
}

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
    /// 从 UTF-8 字节切片构造栈内联音节（超长时截断至 8 字节）
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_double_array_trie_with_tail_compression() {
        let mut raw_keys = vec![
            "a".to_string(),
            "an".to_string(),
            "ang".to_string(),
            "ao".to_string(),
            "bei'jing".to_string(),
            "ni'hao".to_string(),
            "shang'hai".to_string(),
            "xi'an".to_string(),
            "xian".to_string(),
            "zhong'guo".to_string(),
            "wo'men'dou'shi'hao'peng'you".to_string(),
        ];
        raw_keys.sort();

        let values: Vec<u32> = (0..raw_keys.len() as u32).collect();
        let dat = DoubleArrayTrie::build(&raw_keys, &values);
        let reader = DatReader::new(&dat.base, &dat.check, &dat.tail);

        for (idx, k) in raw_keys.iter().enumerate() {
            let match_res = reader.exact_match(k);
            assert_eq!(match_res, Some(idx as u32), "键 {} 精准匹配异常", k);
        }

        assert_eq!(reader.exact_match("unknown"), None);
        assert_eq!(reader.exact_match("zhong"), None);
        assert_eq!(reader.exact_match("bei"), None);
        assert_eq!(reader.exact_match("wo'men'dou'shi"), None);
        // 包含空字节的异常键必须被安全拦截，不得与叶子终止符混淆
        assert_eq!(reader.exact_match("a\0"), None);
        assert_eq!(reader.exact_match("bei'jing\0"), None);
        assert_eq!(reader.exact_match("\0"), None);
    }

    #[test]
    fn test_dat_build_unsorted_and_duplicate_keys() {
        let keys = vec![
            "zhong'guo".to_string(),
            "bei'jing".to_string(),
            "bei'jing".to_string(),
            "an".to_string(),
        ];
        let values = vec![10, 20, 99, 30];
        let dat = DoubleArrayTrie::build(&keys, &values);
        let reader = DatReader::new(&dat.base, &dat.check, &dat.tail);

        assert_eq!(reader.exact_match("an"), Some(30));
        assert_eq!(reader.exact_match("bei'jing"), Some(20));
        assert_eq!(reader.exact_match("zhong'guo"), Some(10));
    }

    #[test]
    fn test_dat_cursor_and_syllable_branches() {
        let keys = vec![
            "wan".to_string(),
            "wan'quan'ke'yi".to_string(),
            "wan'qu".to_string(),
            "wang'luo".to_string(),
        ];
        let values = vec![1, 2, 3, 4];
        let dat = DoubleArrayTrie::build(&keys, &values);
        let reader = DatReader::new(&dat.base, &dat.check, &dat.tail);

        let root = reader.root_cursor();
        let after_wan = root.step_str(&reader, "wan").expect("wan 应存在");
        assert_eq!(after_wan.terminal_value(&reader), Some(1));

        let after_sep = after_wan.step_byte(&reader, b'\'').expect("wan' 应存在");
        let mut branches = Vec::new();
        after_sep.collect_syllable_branches(&reader, "q", 16, &mut branches);
        let branch_syls: Vec<&str> = branches.iter().map(|(s, _)| s.as_str()).collect();
        assert!(branch_syls.contains(&"qu"));
        assert!(branch_syls.contains(&"quan"));

        // 继续沿着 wan'quan 推进到 ke'yi
        let (_, quan_cursor) = branches
            .iter()
            .find(|(s, _)| s == "quan")
            .expect("应找到 quan");
        let after_ke = quan_cursor
            .step_byte(&reader, b'\'')
            .and_then(|c| c.step_str(&reader, "ke"))
            .and_then(|c| c.step_byte(&reader, b'\''))
            .expect("wan'quan'ke' 应存在");
        let mut yi_branches = Vec::new();
        after_ke.collect_syllable_branches(&reader, "y", 16, &mut yi_branches);
        assert_eq!(yi_branches.len(), 1);
        assert_eq!(yi_branches[0].0, "yi");
        assert_eq!(yi_branches[0].1.terminal_value(&reader), Some(2));
    }
}
