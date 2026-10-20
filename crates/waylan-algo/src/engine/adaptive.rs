//! 用户自适应词频学习与预写日志持久化子模型
//!
//! # 设计原理
//! - **实现初衷**：将纯内存词频模型、WAL 预写日志文件句柄以及词库磁盘路径收敛为独立的高内聚子系统。
//! - **核心优势**：解耦算法引擎与具体的词库持久化、半衰期时间戳维护与增量追加逻辑。
//! - **代价与局限**：引擎内部访问词条模型时需经由 `adaptive` 状态机层级转发。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::AlgoError;
use crate::types::ImportedDictEntry;
use crate::user_adaptive::{UserAdaptiveModel, UserAdaptiveWal, UserDictAppendParams};

/// 用户自适应词频学习与预写日志持久化子模型
#[derive(Debug, Default)]
pub struct UserAdaptiveState {
    /// 用户自适应词频纯内存模型
    pub model: UserAdaptiveModel,
    /// 用户词库预写日志持久化管理器
    pub wal: UserAdaptiveWal,
    /// 用户自适应词库持久化文件路径（若设置则每次选词自动追加写盘）
    pub dict_path: Option<PathBuf>,
    /// 最近一次交互/选词的时间戳（秒级 UNIX 时间，用于驱动词频半衰期衰减评估）
    pub last_timestamp: u64,
}

impl UserAdaptiveState {
    /// 创建新的用户自适应状态机
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置用户自适应词库持久化文件路径并自动重放加载已有 WAL 记录
    pub fn set_dict_path(&mut self, path: impl Into<PathBuf>) {
        let p = path.into();
        if p.exists() {
            if let Ok(model) = UserAdaptiveModel::load_or_replay_wal(&p) {
                self.model = model;
            }
        }
        self.dict_path = Some(p);
    }

    /// 记录单次词条上屏并持久化追加至 WAL
    pub fn record_commit(&mut self, text: &str, pinyin: &str, current_timestamp: u64) {
        self.last_timestamp = self.last_timestamp.max(current_timestamp);
        if let Some(ref path) = self.dict_path {
            let _ = self.wal.append(
                &mut self.model,
                &UserDictAppendParams {
                    path,
                    text,
                    pinyin,
                    timestamp: self.last_timestamp,
                },
            );
        } else {
            self.model.record_commit(text, pinyin, self.last_timestamp);
        }
    }

    /// 强制执行用户自适应词库的归并与刷盘持久化 (WAL Compaction)
    pub fn compact(&mut self) -> Result<(), AlgoError> {
        if let Some(ref path) = self.dict_path {
            self.wal.compact(&self.model, path)?;
        }
        Ok(())
    }

    /// 检查并在达到压缩阈值时对用户词库执行归并压缩（通常在会话失焦时调用）
    pub fn maybe_compact(&mut self) -> Result<bool, AlgoError> {
        if let Some(ref path) = self.dict_path {
            self.wal.maybe_compact(&self.model, path)
        } else {
            Ok(false)
        }
    }

    /// 判断磁盘上的 WAL 文件是否与当前持有的常驻追加句柄指向同一文件
    pub fn is_same_wal_file(&self, path: &Path) -> bool {
        self.wal.is_same_wal_file(path)
    }

    /// 批量导入外部词库条目到用户自适应模型
    pub fn import_dict_entries(&mut self, entries: &[ImportedDictEntry]) {
        let sys_now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1_700_000_000);
        let now = sys_now
            .max(self.model.latest_timestamp())
            .max(self.last_timestamp);
        self.last_timestamp = now;
        for entry in entries {
            self.model.record_commit(&entry.text, &entry.pinyin, now);
        }
    }
}
