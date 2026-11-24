pub mod wal;
use crate::error::AlgoError;
use crate::types::{AlgoCandidate, CandidateTier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
pub use wal::{UserAdaptiveWal, UserDictAppendParams, WAL_MAGIC, WAL_MAGIC_V1};
pub use waylan_core::MAX_USER_DICT_BYTES;

/// 用户词条上屏次数记录的参数对象
///
/// 承载记录用户词条上屏次数所需的词条文本、拼音键、记录时间戳与本次累计次数。
pub struct CommitCountParams<'a> {
    /// 词条文本
    pub text: &'a str,
    /// 拼音键序列（如 "ni'hao"）
    pub pinyin: &'a str,
    /// 记录时间戳（秒级 UNIX 时间）
    pub current_timestamp: u64,
    /// 本次累计的上屏次数
    pub count: u32,
}

/// 用户词条使用记录
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserEntry {
    /// 词条文本
    pub text: String,
    /// 拼音键序列（如 "ni'hao"）
    pub pinyin: String,
    /// 累计上屏选择次数
    pub commit_count: u32,
    /// 最后一次使用的时间戳（秒级 UNIX 时间）
    pub last_used_timestamp: u64,
}

/// 用户词对搭配使用记录
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserPairEntry {
    /// 累计搭配使用次数
    pub count: u32,
    /// 最后一次使用的时间戳（秒级 UNIX 时间）
    pub last_used_timestamp: u64,
}

/// 用户自适应词库与动态调频纯内存领域模型 (User Adaptive Model)
///
/// # 设计原理
/// - **实现初衷**：根据用户个人输入历史实现智能词频调优与自造新词记忆，使输入法越用越懂用户。
/// - **核心优势**：结合使用频次与时间半衰期衰减算法，既能快速置顶常用词，又避免一次误选终身置顶。
/// - **持久化分离**：纯内存数据结构，彻底解耦物理文件 I/O 与常驻追加文件描述符，由 [`UserAdaptiveWal`] 专门负责预写日志持久化。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UserAdaptiveModel {
    /// 键为拼音序列，值为该拼音下记录的用户词列表
    pub(crate) entries: HashMap<String, Vec<UserEntry>>,
    /// 用户词对搭配记忆（前驱词 -> 后继词 -> 次数与时间戳）
    #[serde(default)]
    pub(crate) pairs: HashMap<String, HashMap<String, UserPairEntry>>,
    /// 缓存维护的最大使用时间戳（用于实现 O(1) 瞬时获取，消除全表遍历）
    #[serde(alias = "latest_ts", default)]
    pub(crate) max_timestamp: u64,
}

impl UserAdaptiveModel {
    /// 创建新的用户自适应模型
    pub fn new() -> Self {
        Self::default()
    }

    /// 合并另一个模型的所有用户词条记录
    pub fn merge(&mut self, other: &Self) {
        for (py, list) in &other.entries {
            for entry in list {
                self.record_commit_count(&CommitCountParams {
                    text: &entry.text,
                    pinyin: py,
                    current_timestamp: entry.last_used_timestamp,
                    count: entry.commit_count,
                });
            }
        }
    }

    /// 记录一次用户选词上屏动作
    pub fn record_commit(&mut self, text: &str, pinyin: &str, current_timestamp: u64) {
        self.record_commit_count(&CommitCountParams {
            text,
            pinyin,
            current_timestamp,
            count: 1,
        });
    }

    /// 记录指定频次的用户选词上屏动作（用于 WAL 快照重放与频次还原）
    pub fn record_commit_count(&mut self, params: &CommitCountParams<'_>) {
        let CommitCountParams {
            text,
            pinyin,
            current_timestamp,
            count,
        } = *params;

        self.max_timestamp = self.max_timestamp.max(current_timestamp);
        let key = pinyin.to_string();
        let list = self.entries.entry(key).or_default();

        if let Some(existing) = list.iter_mut().find(|e| e.text == text) {
            existing.commit_count = existing.commit_count.saturating_add(count);
            existing.last_used_timestamp = existing.last_used_timestamp.max(current_timestamp);
        } else {
            list.push(UserEntry {
                text: text.to_string(),
                pinyin: pinyin.to_string(),
                commit_count: count.max(1),
                last_used_timestamp: current_timestamp,
            });
        }
    }

    /// 记录一次用户词对搭配（前驱词 -> 当前词），支持 30 天半衰期衰减累计
    pub fn record_pair(&mut self, prev: &str, curr: &str, current_timestamp: u64) {
        if prev.is_empty() || curr.is_empty() {
            return;
        }
        self.max_timestamp = self.max_timestamp.max(current_timestamp);
        let next_map = self.pairs.entry(prev.to_string()).or_default();
        let entry = next_map.entry(curr.to_string()).or_insert(UserPairEntry {
            count: 0,
            last_used_timestamp: current_timestamp,
        });
        let decayed = Self::decayed_pair_count(*entry, current_timestamp).round() as u32;
        entry.count = decayed.saturating_add(1);
        entry.last_used_timestamp = entry.last_used_timestamp.max(current_timestamp);
    }

    /// 计算词对搭配在当前时间戳下的衰减次数
    #[inline]
    fn decayed_pair_count(entry: UserPairEntry, current_timestamp: u64) -> f64 {
        let elapsed_days =
            current_timestamp.saturating_sub(entry.last_used_timestamp) as f64 / 86400.0;
        entry.count as f64 * (0.5_f64).powf(elapsed_days / 30.0)
    }

    /// 查询用户历史词对搭配对当前转移的加分（封顶 45_000，避免单次误选永久压死常用词）
    pub fn pair_boost_score(&self, prev: &str, curr: &str, current_timestamp: u64) -> i64 {
        let Some(entry) = self.pairs.get(prev).and_then(|next| next.get(curr)) else {
            return 0;
        };
        let decayed = Self::decayed_pair_count(*entry, current_timestamp);
        let raw_boost = ((1.0 + decayed).ln() * 18_000.0) as i64;
        raw_boost.min(45_000)
    }

    /// 忘记指定词条及其相关的前后词对搭配（若提供 `pinyin` 则仅删除对应拼音下的用户词条）
    pub fn forget_word(&mut self, text: &str, pinyin: Option<&str>) -> bool {
        if text.is_empty() {
            return false;
        }
        let mut removed = false;
        if let Some(py) = pinyin {
            if let Some(list) = self.entries.get_mut(py) {
                let before = list.len();
                list.retain(|e| e.text != text);
                if list.len() < before {
                    removed = true;
                }
                if list.is_empty() {
                    self.entries.remove(py);
                }
            }
        } else {
            self.entries.retain(|_, list| {
                let before = list.len();
                list.retain(|e| e.text != text);
                if list.len() < before {
                    removed = true;
                }
                !list.is_empty()
            });
        }

        if self.pairs.remove(text).is_some() {
            removed = true;
        }
        for next in self.pairs.values_mut() {
            if next.remove(text).is_some() {
                removed = true;
            }
        }
        self.pairs.retain(|_, next| !next.is_empty());
        removed
    }

    /// 根据用户历史为候选词附加动态习惯提权分
    pub fn apply_boost(&self, candidate: &mut AlgoCandidate, current_timestamp: u64) {
        if candidate.tier == CandidateTier::UserHistory {
            return;
        }
        if let Some(list) = self.entries.get(&candidate.pinyin)
            && let Some(entry) = list.iter().find(|e| e.text == candidate.text)
        {
            let boost = Self::calculate_decay_score(entry, current_timestamp);
            candidate.score = candidate.score.saturating_add(boost);
        }
    }

    /// 查询用户自造新词（基础库未收录但在用户历史中出现过的词）
    pub fn lookup_user_words(&self, pinyin: &str, current_timestamp: u64) -> Vec<AlgoCandidate> {
        let mut results = Vec::new();
        self.lookup_user_words_into(pinyin, current_timestamp, &mut results);
        results
    }

    /// 查询用户自造新词并写入指定的复用容器中（避免堆重新分配）
    pub fn lookup_user_words_into(
        &self,
        pinyin: &str,
        current_timestamp: u64,
        out: &mut Vec<AlgoCandidate>,
    ) {
        if let Some(list) = self.entries.get(pinyin) {
            out.reserve(list.len());
            for entry in list {
                let boost = Self::calculate_decay_score(entry, current_timestamp);
                let score = 5_000_000_u32.saturating_add(boost);
                out.push(
                    AlgoCandidate::new(&entry.text, pinyin, score)
                        .with_tier(CandidateTier::UserHistory),
                );
            }
        }
    }

    /// 基于 30 天半衰期计算时间衰减动态加权得分
    fn calculate_decay_score(entry: &UserEntry, current_timestamp: u64) -> u32 {
        let elapsed_seconds = current_timestamp.saturating_sub(entry.last_used_timestamp);
        let elapsed_days = (elapsed_seconds / 86400) as f64;
        // 半衰期 T = 30 天
        let decay_factor = (0.5_f64).powf(elapsed_days / 30.0);
        let base_weight = entry.commit_count as f64 * 80_000.0;
        (base_weight * decay_factor) as u32
    }

    /// 获取记录的用户词条总数
    pub fn total_entries(&self) -> usize {
        self.entries.values().map(|list| list.len()).sum()
    }

    /// 获取缓存的最大使用时间戳
    #[inline]
    pub fn max_timestamp(&self) -> u64 {
        self.max_timestamp
    }

    /// 获取所有历史记录中最新的使用时间戳（若无记录则返回默认基准时间戳，O(1) 复杂度）
    #[inline]
    pub fn latest_timestamp(&self) -> u64 {
        if self.max_timestamp > 0 {
            self.max_timestamp
        } else {
            1_700_000_000
        }
    }

    /// 清空所有历史自造词与调频数据
    pub fn clear(&mut self) {
        self.entries.clear();
        self.pairs.clear();
        self.max_timestamp = 0;
    }

    /// 从 WAL 日志文件中重放还原用户词库模型，向下兼容旧版全量文件与旧版 V1 WAL
    pub fn load_or_replay_wal<P: AsRef<Path>>(path: P) -> Result<Self, AlgoError> {
        UserAdaptiveWal::load_or_replay(path)
    }

    /// 对 WAL 文件执行压缩归并快照，清除冗余流水记录
    pub fn compact_wal<P: AsRef<Path>>(&self, path: P) -> Result<(), AlgoError> {
        UserAdaptiveWal::compact_model(self, path)
    }
}

#[cfg(test)]
mod tests;
