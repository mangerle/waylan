use crate::error::AlgoError;
use crate::types::{AlgoCandidate, CandidateTier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// 旧版 WAL 预写日志魔数标识（16 字节，V1 记录未携带频次字段）
pub const WAL_MAGIC_V1: &[u8; 16] = b"WAYLAN_USERWAL01";

/// 当前 WAL 预写日志魔数标识（16 字节，V2 记录原生支持 4 字节频次字段）
pub const WAL_MAGIC: &[u8; 16] = b"WAYLAN_USERWAL02";

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

    /// 追加单条用户上屏记录到 WAL（预写日志）中
    ///
    /// # 架构优势
    /// 1. 采用追加写（Append-Only），每次选词仅产生数十字节文件尾部写入，耗时控制在微秒级；
    /// 2. 彻底根除每次选词保存全量序列化造成的磁盘 I/O 停顿。
    pub fn append_to_wal<P: AsRef<Path>>(
        &mut self,
        path: P,
        text: &str,
        pinyin: &str,
        timestamp: u64,
    ) -> Result<(), AlgoError> {
        self.record_commit(text, pinyin, timestamp);

        let path_ref = path.as_ref();
        let mut guard = self
            .wal_file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let need_reopen = match &*guard {
            Some((current_path, _)) => current_path != path_ref,
            None => true,
        };

        if need_reopen {
            if let Some(parent) = path_ref.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }

            // 若目标文件已存在但并非当前 V2 WAL 格式，直接归并升级为标准 V2 WAL 格式
            if path_ref.exists() {
                if let Ok(mut existing_file) = File::open(path_ref) {
                    let len = existing_file.metadata().map(|m| m.len()).unwrap_or(0);
                    if len > 0 {
                        let mut magic = [0u8; 16];
                        if existing_file.read_exact(&mut magic).is_err() || &magic != WAL_MAGIC {
                            drop(existing_file);
                            drop(guard);
                            self.compact_wal(path_ref)?;
                            guard = self
                                .wal_file
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                    }
                }
            }

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path_ref)
                .map_err(|e| AlgoError::UserDictPersistFailed {
                    path: PathBuf::from(path_ref),
                    source: e,
                })?;

            // 文件大小为 0 时写入魔数头
            if file.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
                file.write_all(WAL_MAGIC)
                    .map_err(|e| AlgoError::UserDictPersistFailed {
                        path: PathBuf::from(path_ref),
                        source: e,
                    })?;
            }

            *guard = Some((PathBuf::from(path_ref), file));
        }

        let py_bytes = pinyin.as_bytes();
        let text_bytes = text.as_bytes();
        let py_len = py_bytes.len() as u16;
        let text_len = text_bytes.len() as u16;
        let count: u32 = 1;

        let mut buf = Vec::with_capacity(20 + py_bytes.len() + text_bytes.len());
        buf.extend_from_slice(&timestamp.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&py_len.to_le_bytes());
        buf.extend_from_slice(&text_len.to_le_bytes());
        buf.extend_from_slice(py_bytes);
        buf.extend_from_slice(text_bytes);

        // 简易校验和防部分写入损坏
        let checksum: u32 = buf.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32));
        buf.extend_from_slice(&checksum.to_le_bytes());

        if let Some((_, ref mut file)) = *guard {
            file.write_all(&buf)
                .map_err(|e| AlgoError::UserDictPersistFailed {
                    path: PathBuf::from(path_ref),
                    source: e,
                })?;
        }

        drop(guard);

        // 运行期累计写入流水达到阈值时，自动触发一次原子压缩合并，避免 WAL 持续膨胀
        self.wal_append_counter += 1;
        if self.wal_append_counter >= self.compact_threshold {
            self.compact_wal(path_ref)?;
            self.wal_append_counter = 0;
        }

        Ok(())
    }

    /// 从 WAL 日志文件中重放还原用户词库模型，向下兼容旧版全量文件与旧版 V1 WAL
    pub fn load_or_replay_wal<P: AsRef<Path>>(path: P) -> Result<Self, AlgoError> {
        let path_ref = path.as_ref();
        if !path_ref.exists() {
            return Ok(Self::new());
        }

        let mut file = File::open(path_ref).map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        let mut magic = [0u8; 16];
        if file.read_exact(&mut magic).is_ok() {
            if &magic == WAL_MAGIC {
                let mut model = Self::new();
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)
                    .map_err(|e| AlgoError::DictionaryIo {
                        path: PathBuf::from(path_ref),
                        source: e,
                    })?;

                let mut record_count = 0;
                let mut cursor = 0;
                while cursor + 16 <= buf.len() {
                    let timestamp =
                        u64::from_le_bytes(buf[cursor..cursor + 8].try_into().unwrap_or([0; 8]));
                    let count = u32::from_le_bytes(
                        buf[cursor + 8..cursor + 12].try_into().unwrap_or([0; 4]),
                    );
                    let py_len = u16::from_le_bytes(
                        buf[cursor + 12..cursor + 14].try_into().unwrap_or([0; 2]),
                    ) as usize;
                    let text_len = u16::from_le_bytes(
                        buf[cursor + 14..cursor + 16].try_into().unwrap_or([0; 2]),
                    ) as usize;
                    cursor += 16;

                    if cursor + py_len + text_len + 4 > buf.len() {
                        break;
                    }

                    let py_str = std::str::from_utf8(&buf[cursor..cursor + py_len]).unwrap_or("");
                    cursor += py_len;
                    let text_str =
                        std::str::from_utf8(&buf[cursor..cursor + text_len]).unwrap_or("");
                    cursor += text_len;
                    cursor += 4; // 跳过校验和

                    if !py_str.is_empty() && !text_str.is_empty() {
                        model.record_commit_count(text_str, py_str, timestamp, count);
                        record_count += 1;
                    }
                }

                // 启动加载自动压缩判定：
                // 若记录流水数 >= 50 且流水数超过独立有效词条数的 3 倍，说明存在大量历史冗余流水，自动触发一次原子压缩
                if record_count >= 50 && record_count > model.total_entries() * 3 {
                    let _ = model.compact_wal(path_ref);
                }

                Ok(model)
            } else if &magic == WAL_MAGIC_V1 {
                let mut model = Self::new();
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)
                    .map_err(|e| AlgoError::DictionaryIo {
                        path: PathBuf::from(path_ref),
                        source: e,
                    })?;

                let mut record_count = 0;
                let mut cursor = 0;
                while cursor + 12 <= buf.len() {
                    let timestamp =
                        u64::from_le_bytes(buf[cursor..cursor + 8].try_into().unwrap_or([0; 8]));
                    let py_len = u16::from_le_bytes(
                        buf[cursor + 8..cursor + 10].try_into().unwrap_or([0; 2]),
                    ) as usize;
                    let text_len = u16::from_le_bytes(
                        buf[cursor + 10..cursor + 12].try_into().unwrap_or([0; 2]),
                    ) as usize;
                    cursor += 12;

                    if cursor + py_len + text_len + 4 > buf.len() {
                        break;
                    }

                    let py_str = std::str::from_utf8(&buf[cursor..cursor + py_len]).unwrap_or("");
                    cursor += py_len;
                    let text_str =
                        std::str::from_utf8(&buf[cursor..cursor + text_len]).unwrap_or("");
                    cursor += text_len;
                    cursor += 4; // 跳过校验和

                    if !py_str.is_empty() && !text_str.is_empty() {
                        model.record_commit(text_str, py_str, timestamp);
                        record_count += 1;
                    }
                }

                if record_count >= 50 && record_count > model.total_entries() * 3 {
                    let _ = model.compact_wal(path_ref);
                }

                Ok(model)
            } else {
                Self::load_from_file(path_ref)
            }
        } else {
            Self::load_from_file(path_ref)
        }
    }

    /// 对 WAL 文件执行压缩归并快照，清除冗余流水记录
    pub fn compact_wal<P: AsRef<Path>>(&self, path: P) -> Result<(), AlgoError> {
        let path_ref = path.as_ref();
        // 压缩前主动释放常驻写入句柄，避免占用文件妨碍原子 rename
        if let Ok(mut guard) = self.wal_file.lock() {
            *guard = None;
        }

        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let temp_path = path_ref.with_extension("wal.tmp");

        {
            let mut file =
                File::create(&temp_path).map_err(|e| AlgoError::UserDictPersistFailed {
                    path: temp_path.clone(),
                    source: e,
                })?;

            file.write_all(WAL_MAGIC)
                .map_err(|e| AlgoError::UserDictPersistFailed {
                    path: temp_path.clone(),
                    source: e,
                })?;

            for list in self.entries.values() {
                for entry in list {
                    let py_bytes = entry.pinyin.as_bytes();
                    let text_bytes = entry.text.as_bytes();
                    let py_len = py_bytes.len() as u16;
                    let text_len = text_bytes.len() as u16;

                    let mut buf = Vec::with_capacity(20 + py_bytes.len() + text_bytes.len());
                    buf.extend_from_slice(&entry.last_used_timestamp.to_le_bytes());
                    buf.extend_from_slice(&entry.commit_count.to_le_bytes());
                    buf.extend_from_slice(&py_len.to_le_bytes());
                    buf.extend_from_slice(&text_len.to_le_bytes());
                    buf.extend_from_slice(py_bytes);
                    buf.extend_from_slice(text_bytes);
                    let checksum: u32 = buf.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32));
                    buf.extend_from_slice(&checksum.to_le_bytes());

                    file.write_all(&buf)
                        .map_err(|e| AlgoError::UserDictPersistFailed {
                            path: temp_path.clone(),
                            source: e,
                        })?;
                }
            }
        }

        std::fs::rename(&temp_path, path_ref).map_err(|e| AlgoError::UserDictPersistFailed {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        Ok(())
    }

    /// 持久化保存到本地文件
    ///
    /// # Errors
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

    #[test]
    fn test_wal_append_replay_and_compaction() {
        let temp_dir = std::env::temp_dir();
        let wal_path = temp_dir.join("test_user_history.wal");
        if wal_path.exists() {
            let _ = std::fs::remove_file(&wal_path);
        }

        // 1. 流式追加记录
        let mut model = UserAdaptiveModel::new();
        model
            .append_to_wal(&wal_path, "微澜", "wei'lan", 1_700_000_000)
            .expect("写入第1条WAL");
        model
            .append_to_wal(&wal_path, "微澜", "wei'lan", 1_700_000_100)
            .expect("写入第2条WAL");
        model
            .append_to_wal(&wal_path, "输入法", "shu'ru'fa", 1_700_000_200)
            .expect("写入第3条WAL");

        // 2. 从 WAL 文件重放
        let restored = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("WAL重放");
        assert_eq!(restored.total_entries(), 2);
        let wl_words = restored.lookup_user_words("wei'lan", 1_700_000_200);
        assert_eq!(wl_words[0].text, "微澜");
        let wl_entry = restored
            .entries
            .get("wei'lan")
            .unwrap()
            .iter()
            .find(|e| e.text == "微澜")
            .unwrap();
        assert_eq!(wl_entry.commit_count, 2);

        // 3. 压缩归并
        restored.compact_wal(&wal_path).expect("WAL归并成功");
        let compacted = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("重放归并后的WAL");
        assert_eq!(compacted.total_entries(), 2);
        let compacted_entry = compacted
            .entries
            .get("wei'lan")
            .unwrap()
            .iter()
            .find(|e| e.text == "微澜")
            .unwrap();
        assert_eq!(compacted_entry.commit_count, 2, "压缩后词频必须忠实保留");

        let _ = std::fs::remove_file(wal_path);
    }

    #[test]
    fn test_wal_runtime_auto_compaction() {
        let temp_dir = std::env::temp_dir();
        let wal_path = temp_dir.join("test_runtime_auto_compact.wal");
        if wal_path.exists() {
            let _ = std::fs::remove_file(&wal_path);
        }

        let mut model = UserAdaptiveModel::new();
        // 设置较小阈值为 5，用于测试自动触发
        model.set_compact_threshold(5);

        for i in 0..4 {
            model
                .append_to_wal(&wal_path, "测试", "ce'shi", 1_700_000_000 + i)
                .expect("追加写入");
        }
        assert_eq!(model.wal_append_counter(), 4);

        // 第 5 次追加达到阈值，触发自动 compact 并清零计数器
        model
            .append_to_wal(&wal_path, "测试", "ce'shi", 1_700_000_010)
            .expect("触发自动压缩");
        assert_eq!(model.wal_append_counter(), 0);

        // 重新加载验证数据完整无损
        let restored = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("重放验证");
        assert_eq!(restored.total_entries(), 1);

        let _ = std::fs::remove_file(wal_path);
    }

    #[test]
    fn test_wal_startup_auto_compaction() {
        let temp_dir = std::env::temp_dir();
        let wal_path = temp_dir.join("test_startup_auto_compact.wal");
        if wal_path.exists() {
            let _ = std::fs::remove_file(&wal_path);
        }

        let mut model = UserAdaptiveModel::new();
        model.set_compact_threshold(100_000); // 运行期不触发

        // 写入 60 条同一个词的流水（高冗余率：60 流水 vs 1 个词）
        for i in 0..60 {
            model
                .append_to_wal(&wal_path, "高频词", "gao'pin'ci", 1_700_000_000 + i)
                .expect("追加");
        }

        let size_before = std::fs::metadata(&wal_path).unwrap().len();

        // 启动重放检测到流水数 >= 50 且 > 词条数*3，触发自动压缩
        let restored = UserAdaptiveModel::load_or_replay_wal(&wal_path).expect("重放触发启动压缩");
        assert_eq!(restored.total_entries(), 1);

        let size_after = std::fs::metadata(&wal_path).unwrap().len();
        assert!(
            size_after < size_before,
            "自动压缩后文件体积应明显缩小 ({} -> {})",
            size_before,
            size_after
        );

        let _ = std::fs::remove_file(wal_path);
    }
}
