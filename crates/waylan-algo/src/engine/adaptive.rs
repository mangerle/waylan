//! 用户自适应词频学习与预写日志持久化子模型
//!
//! # 设计原理
//! - **实现初衷**：将纯内存词频模型、WAL 预写日志文件句柄以及词库磁盘路径收敛为独立的高内聚子系统。
//! - **核心优势**：解耦算法引擎与具体的词库持久化、半衰期时间戳维护与增量追加逻辑。
//! - **代价与局限**：引擎内部访问词条模型时需经由 `adaptive` 状态机层级转发。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::AlgoError;
use crate::types::ImportedDictEntry;
use crate::user_adaptive::{AsyncWalWriter, UserAdaptiveModel};

/// 用户自适应词频学习与预写日志持久化子模型
#[derive(Debug, Default)]
pub struct UserAdaptiveState {
    /// 用户自适应词频纯内存模型
    pub model: UserAdaptiveModel,
    /// 用户词库异步预写日志持久化调度器（通过有界通道将磁盘 I/O 移出按键热路径）
    pub wal: AsyncWalWriter,
    /// 用户自适应词库持久化文件路径（若设置则每次选词自动异步追加写盘）
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
        if p.exists()
            && let Ok(model) = UserAdaptiveModel::load_or_replay_wal(&p)
        {
            self.model = model;
        }
        self.dict_path = Some(p);
    }

    /// 记录单次词条上屏：立即更新纯内存模型并异步投递 WAL 落盘
    pub fn record_commit(&mut self, text: &str, pinyin: &str, current_timestamp: u64) {
        self.last_timestamp = self.last_timestamp.max(current_timestamp);
        self.model.record_commit(text, pinyin, self.last_timestamp);
        if let Some(ref path) = self.dict_path {
            self.wal
                .send_append(path, text, pinyin, self.last_timestamp);
        }
    }

    /// 记录用户词对搭配：立即更新纯内存模型并异步投递 WAL 落盘
    pub fn record_pair(&mut self, prev: &str, curr: &str, current_timestamp: u64) {
        if prev.is_empty() || curr.is_empty() {
            return;
        }
        self.last_timestamp = self.last_timestamp.max(current_timestamp);
        self.model.record_pair(prev, curr, self.last_timestamp);
        if let Some(ref path) = self.dict_path {
            self.wal
                .send_append_pair(path, prev, curr, self.last_timestamp);
        }
    }

    /// 忘记指定用户词及其关联词对搭配，并异步写入 WAL 遗忘帧
    pub fn forget_word(
        &mut self,
        text: &str,
        pinyin: Option<&str>,
        current_timestamp: u64,
    ) -> bool {
        if text.is_empty() {
            return false;
        }
        self.last_timestamp = self.last_timestamp.max(current_timestamp);
        let removed = self.model.forget_word(text, pinyin);
        if let Some(ref path) = self.dict_path {
            self.wal
                .send_append_forget(path, text, pinyin, self.last_timestamp);
        }
        removed
    }

    /// 强制执行用户自适应词库的同步归并与刷盘持久化 (WAL Compaction)
    pub fn compact(&mut self) -> Result<(), AlgoError> {
        if let Some(ref path) = self.dict_path {
            self.wal.compact_sync(path, &self.model)?;
        }
        Ok(())
    }

    /// 检查并在达到压缩阈值时异步触发用户词库归并压缩（通常在会话失焦时调用）
    pub fn maybe_compact(&mut self) -> Result<bool, AlgoError> {
        if let Some(ref path) = self.dict_path {
            Ok(self.wal.maybe_compact(path, &self.model))
        } else {
            Ok(false)
        }
    }

    /// 获取最近一次内部 WAL 归并落盘时间戳追踪器
    #[inline]
    pub fn last_compact_time(&self) -> Arc<AtomicU64> {
        self.wal.last_compact_time()
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
