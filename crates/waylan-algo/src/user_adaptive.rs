pub mod wal;
use crate::error::AlgoError;
use crate::types::{AlgoCandidate, CandidateTier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
pub use wal::{WAL_MAGIC, WAL_MAGIC_V1};

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

/// 用户自适应词库与动态调频模型 (User Adaptive Engine)
///
/// # 设计原理
/// - **实现初衷**：根据用户个人输入历史实现智能词频调优与自造新词记忆，使输入法越用越懂用户。
/// - **核心优势**：结合使用频次与时间半衰期衰减算法，既能快速置顶常用词，又避免一次误选终身置顶。
/// - **代价与局限**：需要定期落盘持久化，但通过内存缓存结合退出/定时写盘可消除 I/O 阻塞。
fn default_wal_file() -> std::sync::Mutex<Option<(PathBuf, File)>> {
    std::sync::Mutex::new(None)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserAdaptiveModel {
    /// 键为拼音序列，值为该拼音下记录的用户词列表
    entries: HashMap<String, Vec<UserEntry>>,
    /// 运行期向 WAL 追加记录的计数器（用于触发周期性自动压缩）
    #[serde(skip)]
    wal_append_counter: usize,
    /// 触发自动压缩的追加条数阈值（默认为 1000 条）
    #[serde(skip)]
    compact_threshold: usize,
    /// 常驻保持的 WAL 日志追加写入文件句柄（消除每次选词时的重复 open/stat/close 系统调用）
    #[serde(skip, default = "default_wal_file")]
    wal_file: std::sync::Mutex<Option<(PathBuf, File)>>,
}

impl Clone for UserAdaptiveModel {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            wal_append_counter: self.wal_append_counter,
            compact_threshold: self.compact_threshold,
            wal_file: std::sync::Mutex::new(None),
        }
    }
}

impl Default for UserAdaptiveModel {
    fn default() -> Self {
        Self::new()
    }
}

impl UserAdaptiveModel {
    /// 默认运行时触发自动压缩的追加阈值
    pub const DEFAULT_COMPACT_THRESHOLD: usize = 1000;

    /// 创建新的用户自适应模型
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            wal_append_counter: 0,
            compact_threshold: Self::DEFAULT_COMPACT_THRESHOLD,
            wal_file: std::sync::Mutex::new(None),
        }
    }

    /// 设置运行期触发自动压缩的 WAL 追加流水阈值
    pub fn set_compact_threshold(&mut self, threshold: usize) {
        self.compact_threshold = threshold;
    }

    /// 获取当前未压缩的 WAL 追加流水计数值
    pub fn wal_append_counter(&self) -> usize {
        self.wal_append_counter
    }

    /// 记录一次用户选词上屏动作
    pub fn record_commit(&mut self, text: &str, pinyin: &str, current_timestamp: u64) {
        self.record_commit_count(text, pinyin, current_timestamp, 1);
    }

    /// 记录指定频次的用户选词上屏动作（用于 WAL 快照重放与频次还原）
    pub fn record_commit_count(
        &mut self,
        text: &str,
        pinyin: &str,
        current_timestamp: u64,
        count: u32,
    ) {
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

    /// 根据用户历史为候选词附加动态习惯提权分
    pub fn apply_boost(&self, candidate: &mut AlgoCandidate, current_timestamp: u64) {
        if let Some(list) = self.entries.get(&candidate.pinyin) {
            if let Some(entry) = list.iter().find(|e| e.text == candidate.text) {
                let boost = Self::calculate_decay_score(entry, current_timestamp);
                candidate.score = candidate.score.saturating_add(boost);
                if entry.commit_count >= 2 {
                    candidate.comment = Some("常用习惯".to_string());
                }
            }
        }
    }

    /// 查询用户自造新词（基础库未收录但在用户历史中出现过的词）
    pub fn lookup_user_words(&self, pinyin: &str, current_timestamp: u64) -> Vec<AlgoCandidate> {
        let mut results = Vec::new();
        if let Some(list) = self.entries.get(pinyin) {
            for entry in list {
                let boost = Self::calculate_decay_score(entry, current_timestamp);
                let score = 5_000_000_u32.saturating_add(boost);
                results.push(
                    AlgoCandidate::new(&entry.text, pinyin, score)
                        .with_comment("自造词")
                        .with_tier(CandidateTier::UserHistory),
                );
            }
        }
        results
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

    /// 当文件路径无法写入时返回 [`AlgoError::UserDictPersistFailed`]。
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), AlgoError> {
        let path_ref = path.as_ref();
        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let file = File::create(path_ref).map_err(|e| AlgoError::UserDictPersistFailed {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, self).map_err(|e| AlgoError::UserDictPersistFailed {
            path: PathBuf::from(path_ref),
            source: std::io::Error::other(e.to_string()),
        })?;
        Ok(())
    }

    /// 从本地文件加载用户词库
    ///
    /// # Errors
    /// 当文件读取或反序列化失败时返回 [`AlgoError::DictionaryIo`]。
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        let reader = BufReader::new(file);
        let model: Self =
            bincode::deserialize_from(reader).map_err(|e| AlgoError::DeserializeFailed {
                path: PathBuf::from(path_ref),
                source: e,
            })?;
        Ok(model)
    }

    /// 获取记录的用户词条总数
    pub fn total_entries(&self) -> usize {
        self.entries.values().map(|list| list.len()).sum()
    }

    /// 获取所有历史记录中最新的使用时间戳（若无记录则返回默认基准时间戳）
    pub fn latest_timestamp(&self) -> u64 {
        self.entries
            .values()
            .flat_map(|list| list.iter().map(|e| e.last_used_timestamp))
            .max()
            .unwrap_or(1_700_000_000)
    }

    /// 清空所有历史自造词与调频数据
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
#[path = "user_adaptive_tests.rs"]
mod tests;
