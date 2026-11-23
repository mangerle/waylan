//! 双数组 Trie (Double-Array Trie, DAT) 离线构建器模块
//!
//! # 设计原理
//! - **实现初衷**：将繁重的广度优先状态转移搜索、空闲偏移探测与单链 Tail 压缩构建算法从运行时只读模块隔离，
//!   通过 `dict-builder` 特性独立门控，确保运行时核心零构建负担。
//! - **核心优势**：
//!   1. 运行时二进制体积与编译速度提升；
//!   2. 职责清晰分离：只读在线匹配与离线数据打包互不侵入。

use std::borrow::Cow;
use std::collections::VecDeque;

use crate::dat::{DoubleArrayTrie, pinyin_char_to_code};

/// 从已按字典序排序且去重的拼音键及其关联值索引构建带 Tail 压缩的 DAT
pub fn build_dat(sorted_keys: &[String], values: &[u32]) -> DoubleArrayTrie {
    assert_eq!(
        sorted_keys.len(),
        values.len(),
        "键集合与值集合长度必须严格一致"
    );
    if sorted_keys.is_empty() {
        return DoubleArrayTrie::empty();
    }

    let mut builder = DatBuilder::new(sorted_keys, values);
    if builder.is_empty() {
        return DoubleArrayTrie::empty();
    }

    while let Some((node, start, end, depth)) = builder.pop_next_node() {
        let children = builder.extract_children(start, end, depth);
        if children.is_empty() {
            continue;
        }

        let b = builder.find_base_offset(&children);
        builder.set_base(node, b as i32);
        builder.write_children(ChildWriteParams {
            node,
            base: b,
            depth,
            children: &children,
        });
        builder.advance_free_pos();
    }

    builder.finish()
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

/// 子状态写入参数
///
/// # 设计原理
/// - **实现初衷**：子状态写入需同时携带"父节点、基准偏移、键前缀深度与子键区间"四项构建上下文，
///   平铺传参易与相邻的 `find_base_offset`、`set_base` 调用发生顺序错配。
/// - **核心优势**：以参数对象聚合构建上下文，调用点按名传参即可完成语义绑定。
/// - **代价与局限**：多引入一个结构体类型。
struct ChildWriteParams<'a> {
    /// 父节点在双数组中的位置
    node: usize,
    /// 父节点在 check 数组中的基准偏移
    base: usize,
    /// 当前已消费的键前缀深度
    depth: usize,
    /// 待写入的子键区间集合（起始下标、结束下标、字符编码）
    children: &'a [(usize, usize, usize)],
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
    fn write_children(&mut self, params: ChildWriteParams<'_>) {
        let ChildWriteParams {
            node,
            base,
            depth,
            children,
        } = params;
        for &(code, c_start, c_end) in children {
            let target = base + code;
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
