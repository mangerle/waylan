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
            return Self {
                base: vec![0],
                check: vec![0],
                tail: Vec::new(),
            };
        }

        let key_bytes: Vec<&[u8]> = sorted_keys.iter().map(|s| s.as_bytes()).collect();

        // 初始预分配数组容量，槽位 0 和 1 为系统保留 (1 为根节点)
        let init_cap = (sorted_keys.len() * 2).next_power_of_two().max(65536);
        let mut base = vec![0i32; init_cap];
        let mut check = vec![0u32; init_cap];
        let mut tail = Vec::with_capacity(sorted_keys.len() * 4);
        let mut max_node_pos = 1usize;
        let mut free_pos = 2usize;

        // BFS 广度优先队列：(node_idx, key_start, key_end, depth)
        let mut queue = VecDeque::with_capacity(32768);
        queue.push_back((1usize, 0usize, sorted_keys.len(), 0usize));

        while let Some((node, start, end, depth)) = queue.pop_front() {
            // 1. 提取当前前缀下的所有子分支字符区间
            let mut children: Vec<(usize, usize, usize)> = Vec::with_capacity(8);
            let mut i = start;
            while i < end {
                let code = if depth < key_bytes[i].len() {
                    pinyin_char_to_code(key_bytes[i][depth])
                } else {
                    0 // 达到键末尾，产生终止符分支
                };

                let mut j = i + 1;
                while j < end {
                    let next_code = if depth < key_bytes[j].len() {
                        pinyin_char_to_code(key_bytes[j][depth])
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

            if children.is_empty() {
                continue;
            }

            // 2. 寻找可无冲突容纳所有 children 的 base 基准偏移量
            let min_code = children[0].0;
            let mut b = if free_pos > min_code {
                free_pos - min_code
            } else {
                1
            };

            loop {
                let mut fit = true;
                for &(code, _, _) in &children {
                    let target = b + code;
                    if target < 2 {
                        fit = false;
                        break;
                    }
                    if target >= check.len() {
                        let new_len = (target + 1).next_power_of_two().max(check.len() * 2);
                        check.resize(new_len, 0);
                        base.resize(new_len, 0);
                    }
                    if check[target] != 0 {
                        fit = false;
                        break;
                    }
                }
                if fit {
                    break;
                }
                b += 1;
            }

            // 3. 确定当前节点的 base 属性
            base[node] = b as i32;

            // 4. 将所有子状态写入 check 校验并入队或写入 Tail
            for &(code, c_start, c_end) in &children {
                let target = b + code;
                check[target] = node as u32;
                if target > max_node_pos {
                    max_node_pos = target;
                }

                if code == 0 {
                    // 终止符命中：叶子状态使用负值存储关联值 -(val + 1)
                    let val = values[c_start];
                    base[target] = -(val as i32 + 1);
                } else if c_end - c_start == 1 {
                    // 触发 Tail 压缩：当前子前缀下仅有唯一一条键，无需继续拆分状态
                    // 将剩余所有字符 + 终止符 0 + 4字节关联值 打包写入 tail
                    let tail_offset = tail.len();
                    let rem_bytes = &key_bytes[c_start][depth + 1..];
                    tail.extend_from_slice(rem_bytes);
                    tail.push(0); // 终止符
                    tail.extend_from_slice(&values[c_start].to_le_bytes()); // 4 字节整数值

                    // target 节点的 base 存储负偏移 -(tail_offset + 1)
                    base[target] = -(tail_offset as i32 + 1);
                } else {
                    // 存在多个键共享分支，继续推进队列
                    queue.push_back((target, c_start, c_end, depth + 1));
                }
            }

            // 维护下一个潜在空闲槽位起点
            while free_pos < check.len() && check[free_pos] != 0 {
                free_pos += 1;
            }
        }

        // 截断尾部未使用的多余数组空间
        check.truncate(max_node_pos + 1);
        base.truncate(max_node_pos + 1);

        Self { base, check, tail }
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
            if code >= 28 {
                return None; // 包含非合法拼音字符
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
                let tail_offset = (-next_b_val - 1) as usize;
                if tail_offset >= self.tail.len() {
                    return None;
                }

                // 剩余待匹配字符
                let remaining = &key_bytes[depth + 1..];
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
            Some((-val - 1) as u32)
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
    }
}
