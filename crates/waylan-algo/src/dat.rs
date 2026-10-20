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
    /// 离线构建逻辑委托给 [`crate::builder::dat_builder::build_dat`]。
    #[cfg(any(feature = "dict-builder", test))]
    pub fn build(sorted_keys: &[String], values: &[u32]) -> Self {
        crate::builder::dat_builder::build_dat(sorted_keys, values)
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
}
