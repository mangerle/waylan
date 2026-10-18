//! 语料流式并行累加与分片调度器
//!
//! # 设计原理
//! - **实现初衷**：统一维护流式分词与词频统计的全局词表、一元词频及分片缓冲映射表。
//! - **核心优势**：在语料读取达到阈值时自动触发局部排序与刷盘，将内存占用严格限定在安全水位以内。

use jieba_rs::Jieba;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::cleaner::Cleaner;
use super::merger::flush_chunk_to_disk;
use waylan_algo::vocab::Vocabulary;

/// 流式统计状态上下文
pub struct CorpusAccumulator {
    pub vocab: Vocabulary,
    pub global_unigram: HashMap<u32, u32>,
    pub current_chunk_map: HashMap<(u32, u32), u32>,
    pub chunk_files: Vec<PathBuf>,
    pub chunk_index: usize,
}

impl CorpusAccumulator {
    /// 消费一批语料行并更新一元/二元词频表
    pub fn consume_batch(
        &mut self,
        lines_to_process: &[String],
        is_sft: bool,
        cleaner_jieba: &(Arc<Cleaner>, Arc<Jieba>),
    ) {
        let (cleaner_ref, jieba_ref) = cleaner_jieba;
        let local_pairs_list: Vec<Vec<(String, String, u32)>> = lines_to_process
            .par_iter()
            .map(|raw_line| cleaner_ref.extract_line_pairs(raw_line, is_sft, jieba_ref))
            .collect();

        for local_pairs in local_pairs_list {
            for (w1, w2, weight) in local_pairs {
                let id1 = self.vocab.insert_or_get(&w1);
                let id2 = self.vocab.insert_or_get(&w2);
                let u_entry = self.global_unigram.entry(id1).or_insert(0);
                *u_entry = u_entry.saturating_add(weight);
                let p_entry = self.current_chunk_map.entry((id1, id2)).or_insert(0);
                *p_entry = p_entry.saturating_add(weight);
            }
        }
    }

    /// 检查并按需将内存中的分片刷写至磁盘
    pub fn maybe_flush_chunk(&mut self, temp_chunk_dir: &Path, limit: usize) {
        if self.current_chunk_map.len() >= limit {
            println!(
                "\n   -> [内存控制] 触发局部剪枝与分片刷盘 (分片 #{}) ...",
                self.chunk_index
            );
            let chunk_path = flush_chunk_to_disk(
                temp_chunk_dir,
                self.chunk_index,
                &mut self.current_chunk_map,
            )
            .expect("刷写临时分片文件失败");
            self.chunk_index = self.chunk_index.saturating_add(1);
            self.chunk_files.push(chunk_path);
        }
    }
}
