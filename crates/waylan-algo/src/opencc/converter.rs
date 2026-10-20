//! 简繁转换正向最大匹配（FMM）与候选词转换引擎

use std::str::CharIndices;

use super::OpenCcDict;
use crate::types::AlgoCandidate;

/// FMM 单次匹配允许的最大窗口字符数上限
const MAX_FMM_WINDOW: usize = 24;

impl OpenCcDict {
    /// 使用正向最大匹配（FMM）将输入简体文本转换为繁体文本
    pub fn convert_text(&self, text: &str) -> String {
        if text.is_empty() || text.is_ascii() {
            return text.to_string();
        }

        let mut result = String::with_capacity(text.len());
        let mut pos = 0usize;

        while pos < text.len() {
            let rem = &text[pos..];
            let mut chars_iter = rem.char_indices();
            let Some((_, first_ch)) = chars_iter.next() else {
                break;
            };

            let mut matched_phrase = false;
            if let Some(&max_len) = self.first_char_max_len.get(&first_ch) {
                if max_len >= 2 {
                    // 使用栈固定数组收集 UTF-8 字节结束偏移，彻底消除每次字符匹配时的堆内存分配
                    if let Some((trad_phrase, byte_end)) =
                        self.match_phrase(rem, chars_iter, max_len)
                    {
                        result.push_str(trad_phrase);
                        pos += byte_end;
                        matched_phrase = true;
                    }
                }
            }

            if !matched_phrase {
                let mapped_ch = self
                    .chars
                    .get(&first_ch)
                    .and_then(|list| list.first().copied())
                    .unwrap_or(first_ch);
                result.push(mapped_ch);
                pos += first_ch.len_utf8();
            }
        }

        result
    }

    /// 以正向最大匹配（FMM）在剩余文本中命中词组转换，返回命中的繁体文本与推进的字节数
    fn match_phrase(
        &self,
        rem: &str,
        chars_iter: CharIndices,
        max_len: usize,
    ) -> Option<(&str, usize)> {
        let mut end_offsets = [0usize; MAX_FMM_WINDOW];
        let offset_count =
            Self::collect_window_ends(chars_iter, max_len.min(MAX_FMM_WINDOW), &mut end_offsets);

        // 从最长窗口向下递减执行零拷贝切片匹配
        for &byte_end in end_offsets[..offset_count].iter().rev() {
            let slice = &rem[..byte_end];
            if let Some(trad_phrase) = self.lookup_phrase(slice) {
                return Some((trad_phrase, byte_end));
            }
        }

        None
    }

    /// 收集候选匹配窗口的 UTF-8 字节结束偏移，返回实际收集到的窗口数量
    fn collect_window_ends(
        chars_iter: CharIndices,
        limit: usize,
        end_offsets: &mut [usize; MAX_FMM_WINDOW],
    ) -> usize {
        let mut offset_count = 0usize;

        for (char_idx, (byte_idx, ch)) in chars_iter.enumerate() {
            let cur_char_count = char_idx + 2;
            if offset_count < MAX_FMM_WINDOW {
                end_offsets[offset_count] = byte_idx + ch.len_utf8();
                offset_count += 1;
            }
            if cur_char_count >= limit {
                break;
            }
        }

        offset_count
    }

    /// 对候选词列表执行简转繁处理：
    /// - 多字词候选（字数 >= 2）：按 FMM 正向最大匹配唯一消歧转换；
    /// - 单字候选（字数 == 1）：首选繁体字原位替换，若存在一简对多繁备选字（如 发 -> 發/髮、后 -> 後/后），自动派生备选单字候选。
    pub fn convert_candidates(&self, candidates: &mut Vec<AlgoCandidate>) {
        let mut converted = Vec::with_capacity(candidates.len() + 4);

        for mut cand in candidates.drain(..) {
            let mut char_iter = cand.text.chars();
            if let (Some(simp_ch), None) = (char_iter.next(), char_iter.next()) {
                if let Some(trad_list) = self.chars.get(&simp_ch) {
                    if let Some(&primary) = trad_list.first() {
                        let base_cand = cand.clone();
                        cand.text = primary.to_string();
                        converted.push(cand);

                        for (idx, &alt_ch) in trad_list.iter().skip(1).enumerate() {
                            if alt_ch != primary {
                                let mut derived = base_cand.clone();
                                derived.text = alt_ch.to_string();
                                derived.score =
                                    base_cand.score.saturating_sub(((idx as u32) + 1) * 10);
                                converted.push(derived);
                            }
                        }
                        continue;
                    }
                }
                converted.push(cand);
            } else {
                cand.text = self.convert_text(&cand.text);
                if cand.is_expandable && !cand.sub_candidates.is_empty() {
                    for sub in &mut cand.sub_candidates {
                        *sub = self.convert_text(sub);
                    }
                }
                converted.push(cand);
            }
        }

        *candidates = converted;
    }
}
