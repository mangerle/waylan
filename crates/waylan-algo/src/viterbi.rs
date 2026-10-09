use crate::table::LexiconTable;
use crate::types::{AlgoCandidate, CandidateTier};

/// 节点状态项（动态规划状态格点）
#[derive(Debug, Clone)]
struct ViterbiNode {
    /// 匹配的汉字词条
    text: String,
    /// 拼音键
    pinyin_key: String,
    /// 累计最优路径得分
    total_score: u32,
    /// 回溯指针：上一个节点的索引
    prev_idx: Option<usize>,
}

/// 维特比整句动态规划解码器 (Viterbi Sentence Decoder)
///
/// # 设计原理
/// - **实现初衷**：对齐 Rime 核心算法思想，采用隐马尔可夫模型（HMM）与动态规划，
///   在音节切分网（Lattice）上求解全局最优连词路径，实现长句拼音一次性打出整句话。
/// - **核心优势**：杜绝局部贪心失准，自动权衡“长词 vs 多个高频短词”，极大提升整句打字首选准确率。
/// - **代价与局限**：对长音节序列（>15 音节）会产生状态空间展开，但通过最大词长剪枝将时间复杂度控制在 O(N)。
#[derive(Debug, Default)]
pub struct ViterbiDecoder;

impl ViterbiDecoder {
    /// 对给定的音节切分列表进行整句动态规划求解
    pub fn decode_sentence(syllables: &[String], table: &LexiconTable) -> Option<AlgoCandidate> {
        let n = syllables.len();
        if n == 0 {
            return None;
        }

        // dp[i] 保存到达第 i 个音节结尾时的候选节点集合
        let mut dp: Vec<Vec<ViterbiNode>> = vec![Vec::new(); n + 1];

        // 虚拟起始点
        dp[0].push(ViterbiNode {
            text: String::new(),
            pinyin_key: String::new(),
            total_score: 0,
            prev_idx: None,
        });

        // 沿音节序列向右推进
        for start_idx in 0..n {
            if dp[start_idx].is_empty() {
                continue;
            }

            let prev_scores: Vec<(usize, u32)> = dp[start_idx]
                .iter()
                .enumerate()
                .map(|(i, node)| (i, node.total_score))
                .collect();

            // 最长中文词一般不超过 4~6 字，限制最大向前跨度
            let max_span = (n - start_idx).min(5);
            for span in 1..=max_span {
                let end_idx = start_idx + span;
                let sub_syllables = &syllables[start_idx..end_idx];
                let key = sub_syllables.join("'");

                let matches = table.lookup(&key);
                if matches.is_empty() {
                    continue;
                }

                // 取匹配中得分最高的前 3 个词参与 DP 转移
                for candidate in matches.iter().take(3) {
                    let word_len = candidate.text.chars().count() as u32;
                    let length_bonus = word_len.saturating_mul(50_000);
                    let edge_score = candidate.score.saturating_add(length_bonus);

                    for &(prev_i, prev_score) in &prev_scores {
                        let candidate_total = prev_score.saturating_add(edge_score);

                        dp[end_idx].push(ViterbiNode {
                            text: candidate.text.clone(),
                            pinyin_key: key.clone(),
                            total_score: candidate_total,
                            prev_idx: Some(prev_i),
                        });
                    }
                }
            }
        }

        // 终点回溯：寻找到达第 n 个音节的最高分节点
        let end_nodes = &dp[n];
        let best_node = end_nodes.iter().max_by_key(|node| node.total_score)?;

        // 回溯收集完整词链
        let mut sentence_parts = Vec::new();
        let mut curr_end = n;
        let mut curr_node_idx = end_nodes
            .iter()
            .position(|node| std::ptr::eq(node, best_node))?;

        while curr_end > 0 {
            let node = &dp[curr_end][curr_node_idx];
            sentence_parts.push(node.text.clone());
            let next_end = curr_end - (node.pinyin_key.split('\'').count());
            let prev_idx = node.prev_idx?;
            curr_end = next_end;
            curr_node_idx = prev_idx;
        }

        sentence_parts.reverse();
        let full_text = sentence_parts.concat();
        let full_pinyin = syllables.join("'");

        // 整句候选标记为 SentenceViterbi 优先级层级
        let sentence_score = best_node.total_score;
        Some(
            AlgoCandidate::new(full_text, full_pinyin, sentence_score)
                .with_comment("整句预测")
                .with_tier(CandidateTier::SentenceViterbi),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viterbi_single_word() {
        let table = LexiconTable::with_sample_data();
        let syllables = vec!["ni".to_string(), "hao".to_string()];
        let best = ViterbiDecoder::decode_sentence(&syllables, &table).expect("应解码成功");
        assert_eq!(best.text, "你好");
        assert!(best.comment.is_some());
    }

    #[test]
    fn test_viterbi_long_sentence() {
        let mut table = LexiconTable::with_sample_data();
        table.insert("lai", "来", 8_000);
        table.insert("le", "了", 8_500);

        let syllables = vec![
            "wo".to_string(),
            "men".to_string(),
            "lai".to_string(),
            "le".to_string(),
        ];
        let best = ViterbiDecoder::decode_sentence(&syllables, &table).expect("应解码成功");
        assert_eq!(best.text, "我们来了");
    }
}
