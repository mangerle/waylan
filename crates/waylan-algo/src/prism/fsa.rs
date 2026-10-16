use crate::syllables::STANDARD_SYLLABLES;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
struct FsaNode {
    next: [u16; 26],
    syllable: Option<&'static str>,
}

/// 编译型音节投影有限状态机 (Prism FSA)
///
/// # 设计原理
/// - 预编译所有 410 个标准普通话音节为紧凑字典树状态机；
/// - 状态转移在 O(1) 数组索引内步进完成，彻底消灭每个前缀的重复二分查找；
/// - 在输入流上单向步进，以严格 O(L) 复杂度提取所有合法音节切分。
pub struct PrismFsa {
    nodes: Vec<FsaNode>,
}

impl PrismFsa {
    /// 全局静态编译单例
    pub fn global() -> &'static PrismFsa {
        static INSTANCE: OnceLock<PrismFsa> = OnceLock::new();
        INSTANCE.get_or_init(Self::compile)
    }

    /// 编译全部合法标准拼音音节
    fn compile() -> Self {
        let mut nodes = vec![FsaNode {
            next: [0; 26],
            syllable: None,
        }];

        for &syl in STANDARD_SYLLABLES {
            let mut curr = 0;
            for b in syl.bytes() {
                if b.is_ascii_lowercase() {
                    let idx = (b - b'a') as usize;
                    let next_state = nodes[curr].next[idx];
                    if next_state == 0 {
                        let new_state = nodes.len() as u16;
                        nodes[curr].next[idx] = new_state;
                        nodes.push(FsaNode {
                            next: [0; 26],
                            syllable: None,
                        });
                        curr = new_state as usize;
                    } else {
                        curr = next_state as usize;
                    }
                }
            }
            nodes[curr].syllable = Some(syl);
        }

        Self { nodes }
    }

    /// 在输入串上以单向步进方式匹配所有合法音节（返回 (标准音节, 匹配字节长度)）
    pub fn match_syllables(&self, input: &str, start: usize) -> Vec<(&'static str, usize)> {
        let mut matches = Vec::new();
        let bytes = input.as_bytes();
        let mut curr = 0;
        let mut len = 0;

        for &b in &bytes[start..] {
            if !b.is_ascii_lowercase() {
                break;
            }
            let idx = (b - b'a') as usize;
            let next_state = self.nodes[curr].next[idx];
            if next_state == 0 {
                break;
            }
            curr = next_state as usize;
            len += 1;
            if let Some(syl) = self.nodes[curr].syllable {
                matches.push((syl, len));
            }
        }
        matches
    }
}
