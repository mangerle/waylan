use crate::error::AlgoError;
use crate::types::{AlgoCandidate, CandidateTier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

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
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UserAdaptiveModel {
    /// 键为拼音序列，值为该拼音下记录的用户词列表
    entries: HashMap<String, Vec<UserEntry>>,
}

impl UserAdaptiveModel {
    /// 创建新的用户自适应模型
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// 记录一次用户选词上屏动作
    pub fn record_commit(&mut self, text: &str, pinyin: &str, current_timestamp: u64) {
        let key = pinyin.to_string();
        let list = self.entries.entry(key).or_default();

        if let Some(existing) = list.iter_mut().find(|e| e.text == text) {
            existing.commit_count = existing.commit_count.saturating_add(1);
            existing.last_used_timestamp = current_timestamp;
        } else {
            list.push(UserEntry {
                text: text.to_string(),
                pinyin: pinyin.to_string(),
                commit_count: 1,
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

    /// 持久化保存到本地文件
    ///
    /// # Errors
    /// 当文件路径无法写入时返回 [`AlgoError::UserDictPersistFailed`]。
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<(), AlgoError> {
        let path_ref = path.as_ref();
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

    /// 清空所有历史自造词与调频数据
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_boost() {
        let mut model = UserAdaptiveModel::new();
        let now = 1_700_000_000;
        model.record_commit("先", "xian", now);
        model.record_commit("先", "xian", now + 10);

        let mut candidate = AlgoCandidate::new("先", "xian", 7_200);
        model.apply_boost(&mut candidate, now + 100);

        assert!(candidate.score > 100_000);
        assert_eq!(candidate.comment.as_deref(), Some("常用习惯"));
    }

    #[test]
    fn test_half_life_decay() {
        let entry = UserEntry {
            text: "测试".to_string(),
            pinyin: "ce'shi".to_string(),
            commit_count: 10,
            last_used_timestamp: 1_000_000,
        };
        let score_immediate = UserAdaptiveModel::calculate_decay_score(&entry, 1_000_000);
        // 30 天后 (30 * 86400 = 2,592,000)
        let score_30_days = UserAdaptiveModel::calculate_decay_score(&entry, 1_000_000 + 2_592_000);

        assert!((score_30_days as f64 - (score_immediate as f64 * 0.5)).abs() < 1000.0);
    }

    #[test]
    fn test_total_entries_and_clear() {
        let mut model = UserAdaptiveModel::new();
        assert_eq!(model.total_entries(), 0);
        model.record_commit("测试", "ce'shi", 100);
        model.record_commit("模型", "mo'xing", 100);
        assert_eq!(model.total_entries(), 2);
        model.clear();
        assert_eq!(model.total_entries(), 0);
    }
}
