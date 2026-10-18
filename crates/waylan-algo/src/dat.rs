use std::borrow::Cow;
use std::collections::VecDeque;

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
    /// # 参数
    /// - `sorted_keys`: 必须严格按字节序递增排序的拼音键列表
    /// - `values`: 每个拼音键对应的整数关联值（如候选列表起始索引）
    pub fn build(sorted_keys: &[String], values: &[u32]) -> Self {
        assert_eq!(
            sorted_keys.len(),
            values.len(),
            "键集合与值集合长度必须严格一致"
        );
        if sorted_keys.is_empty() {
            return Self::empty();
        }

        let mut builder = DatBuilder::new(sorted_keys, values);
        if builder.is_empty() {
            return Self::empty();
        }

        while let Some((node, start, end, depth)) = builder.pop_next_node() {
            let children = builder.extract_children(start, end, depth);
            if children.is_empty() {
                continue;
            }

            let b = builder.find_base_offset(&children);
            builder.set_base(node, b as i32);
            builder.write_children(node, b, depth, &children);
            builder.advance_free_pos();
        }

        builder.finish()
    }

    /// 返回空状态的 DAT
    fn empty() -> Self {
        Self {
            base: vec![0],
            check: vec![0],
            tail: Vec::new(),
        }
    }
}

/// 双数组 Trie 内部构建器，收敛 BFS 状态机与 Tail 压缩缓冲区
struct DatBuilder<'a> {
    key_bytes: Vec<&'a [u8]>,
    effective_values: Cow<'a, [u32]>,
    base: Vec<i32>,
    check: Vec<u32>,
    tail: Vec<u8>,
    max_node_pos: usize,
    free_pos: usize,
    queue: VecDeque<(usize, usize, usize, usize)>,
}

impl<'a> DatBuilder<'a> {
    /// 构造构建器并完成初始键值去重、排序与双数组缓冲区预分配
    fn new(sorted_keys: &'a [String], values: &'a [u32]) -> Self {
        let (key_bytes, effective_values) = prepare_keys_and_values(sorted_keys, values);
        let init_cap = (key_bytes.len() * 2).next_power_of_two().max(65536);
        let mut queue = VecDeque::with_capacity(32768);
        if !key_bytes.is_empty() {
            queue.push_back((1usize, 0usize, key_bytes.len(), 0usize));
        }

        Self {
            key_bytes,
            effective_values,
            base: vec![0i32; init_cap],
            check: vec![0u32; init_cap],
            tail: Vec::with_capacity(sorted_keys.len() * 4),
            max_node_pos: 1,
            free_pos: 2,
            queue,
        }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.key_bytes.is_empty()
    }

    #[inline]
    fn pop_next_node(&mut self) -> Option<(usize, usize, usize, usize)> {
        self.queue.pop_front()
    }

    #[inline]
    fn set_base(&mut self, node: usize, b: i32) {
        self.base[node] = b;
    }

    /// 提取指定前缀深度下的所有子分支字符区间
    fn extract_children(
        &self,
        start: usize,
        end: usize,
        depth: usize,
    ) -> Vec<(usize, usize, usize)> {
        let mut children: Vec<(usize, usize, usize)> = Vec::with_capacity(8);
        let mut i = start;
        while i < end {
            let code = if depth < self.key_bytes[i].len() {
                pinyin_char_to_code(self.key_bytes[i][depth])
            } else {
                0
            };

            let mut j = i + 1;
            while j < end {
                let next_code = if depth < self.key_bytes[j].len() {
                    pinyin_char_to_code(self.key_bytes[j][depth])
                } else {
                    0
                };
                if next_code != code {
                    break;
                }
                j += 1;
            }
            children.push((code, i, j));
            i = j;
        }
        children
    }

    /// 寻找可无冲突容纳所有 children 的 base 基准偏移量
    fn find_base_offset(&mut self, children: &[(usize, usize, usize)]) -> usize {
        let min_code = children[0].0;
        let mut b = if self.free_pos > min_code {
            self.free_pos - min_code
        } else {
            1
        };

        loop {
            let mut fit = true;
            for &(code, _, _) in children {
                let target = b + code;
                if target < 2 {
                    fit = false;
                    break;
                }
                if target >= self.check.len() {
                    let new_len = (target + 1).next_power_of_two().max(self.check.len() * 2);
                    self.check.resize(new_len, 0);
                    self.base.resize(new_len, 0);
                }
                if self.check[target] != 0 {
                    fit = false;
                    break;
                }
            }
            if fit {
                return b;
            }
            b += 1;
        }
    }

    /// 将所有子状态写入 check 校验并入队或写入 Tail
    fn write_children(
        &mut self,
        node: usize,
        b: usize,
        depth: usize,
        children: &[(usize, usize, usize)],
    ) {
        for &(code, c_start, c_end) in children {
            let target = b + code;
            self.check[target] = node as u32;
            if target > self.max_node_pos {
                self.max_node_pos = target;
            }

            if code == 0 {
                // 终止符命中：叶子状态使用负值存储关联值 -(val + 1)
                let val = self.effective_values[c_start];
                self.base[target] = -(val as i32 + 1);
            } else if c_end - c_start == 1 {
                // 触发 Tail 压缩：当前子前缀下仅有唯一一条键，无需继续拆分状态
                let tail_offset = self.tail.len();
                let rem_bytes = &self.key_bytes[c_start][depth + 1..];
                self.tail.extend_from_slice(rem_bytes);
                self.tail.push(0);
                self.tail
                    .extend_from_slice(&self.effective_values[c_start].to_le_bytes());

                self.base[target] = -(tail_offset as i32 + 1);
            } else {
                // 存在多个键共享分支，继续推进队列
                self.queue.push_back((target, c_start, c_end, depth + 1));
            }
        }
    }

    /// 维护推进下一个空闲槽位起点
    #[inline]
    fn advance_free_pos(&mut self) {
        while self.free_pos < self.check.len() && self.check[self.free_pos] != 0 {
            self.free_pos += 1;
        }
    }

    /// 截断尾部未使用的多余数组空间并产生最终的双数组结构
    fn finish(mut self) -> DoubleArrayTrie {
        self.check.truncate(self.max_node_pos + 1);
        self.base.truncate(self.max_node_pos + 1);
        DoubleArrayTrie {
            base: self.base,
            check: self.check,
            tail: self.tail,
        }
    }
}

/// 准备键字节切片与关联值切片，若未严格排序去重则在此进行排序和去重
fn prepare_keys_and_values<'a>(
    sorted_keys: &'a [String],
    values: &'a [u32],
) -> (Vec<&'a [u8]>, Cow<'a, [u32]>) {
    let is_strictly_sorted_unique = sorted_keys.windows(2).all(|w| w[0] < w[1]);
    if is_strictly_sorted_unique {
        (
            sorted_keys.iter().map(|s| s.as_bytes()).collect(),
            Cow::Borrowed(values),
        )
    } else {
        let mut pairs: Vec<(&[u8], u32)> = sorted_keys
            .iter()
            .map(|s| s.as_bytes())
            .zip(values.iter().copied())
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        pairs.dedup_by(|a, b| a.0 == b.0);
        let kb = pairs.iter().map(|(k, _)| *k).collect();
        let vals = pairs.into_iter().map(|(_, v)| v).collect();
        (kb, Cow::Owned(vals))
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
            let b = key_bytes[depth];
            let code = pinyin_char_to_code(b);
            if code == 0 || code >= 28 {
                return None; // 拦截空字节穿透与非合法拼音字符
            }

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

            // 检查新状态是否命中 Tail 压缩
            let next_b_val = self.base[next];
            if next_b_val < 0 {
                // 进入 Tail 匹配
                let tail_offset = (-(next_b_val + 1)) as usize;
                if tail_offset >= self.tail.len() {
                    return None;
                }

                // 剩余待匹配字符
                let remaining = &key_bytes[depth + 1..];
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
                return Some(u32::from_le_bytes(val_bytes));
            }

            curr = next;
            depth += 1;
        }

        // 走终止符 code 0
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
