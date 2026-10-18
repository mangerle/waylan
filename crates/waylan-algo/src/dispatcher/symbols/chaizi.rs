//! U 模式拆字与叠字查询求值器
//!
//! # 设计原理
//! - **实现初衷**：为用户输入 `u` 引导的前缀提供生僻汉字与拼字（如双叠、三叠、四叠、拆字组合）反查能力。
//! - **核心优势**：纯查询算法，数据表解耦外置至 `chaizi_data.rs`，支持严格前缀匹配与动态候选词打分。
//! - **代价与局限**：线性扫描静态表，因表项规模较小（几十项），耗时在微秒级。

use crate::dispatcher::symbols::chaizi_data::CHAIZI_TABLE;
use crate::types::AlgoCandidate;

/// 查询 U 模式拆字与叠字候选列表
pub(crate) fn query_u_mode(input: &str) -> Vec<AlgoCandidate> {
    let mut candidates = Vec::with_capacity(8);
    for entry in CHAIZI_TABLE {
        if entry.code == input {
            candidates.push(
                AlgoCandidate::new(entry.text, format!("u{}", entry.code), 20_000)
                    .with_comment(entry.comment),
            );
        } else if entry.code.starts_with(input) {
            candidates.push(
                AlgoCandidate::new(entry.text, format!("u{}", entry.code), 10_000)
                    .with_comment(entry.comment),
            );
        }
    }
    candidates.sort_by_key(|c| std::cmp::Reverse(c.score));
    candidates
}
