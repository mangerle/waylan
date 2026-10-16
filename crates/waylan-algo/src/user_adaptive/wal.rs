use crate::error::AlgoError;
use crate::user_adaptive::UserAdaptiveModel;
use log::warn;
use rustix::fs::{flock, FlockOperation};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 旧版 WAL 预写日志魔数标识（16 字节，V1 记录未携带频次字段）
pub const WAL_MAGIC_V1: &[u8; 16] = b"WAYLAN_USERWAL01";

/// 当前 WAL 预写日志魔数标识（16 字节，V2 记录原生支持 4 字节频次字段）
pub const WAL_MAGIC: &[u8; 16] = b"WAYLAN_USERWAL02";

/// 校验和初始种子（非零常数，防止全零填充块通过校验和检查）
pub const WAL_CHECKSUM_SEED: u32 = 0x5a5a_a5a5;

static LAST_INTERNAL_COMPACT_MS: AtomicU64 = AtomicU64::new(0);

/// 查询当前是否处于进程内部刚刚完成 WAL 归并落盘的防重保护窗口内（毫秒）
pub fn is_recent_internal_compact(window_ms: u64) -> bool {
    let last = LAST_INTERNAL_COMPACT_MS.load(Ordering::Acquire);
    if last == 0 {
        return false;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now.saturating_sub(last) <= window_ms
}

/// 标记内部 WAL 归并压缩落盘完成
pub fn mark_internal_compact() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    LAST_INTERNAL_COMPACT_MS.store(now, Ordering::Release);
}

impl UserAdaptiveModel {
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
        let path_ref = path.as_ref();
        let mut guard = self
            .wal_file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let need_reopen = match &*guard {
            Some((current_path, opened_file)) => {
                if current_path != path_ref {
                    true
                } else {
                    match (opened_file.metadata(), std::fs::metadata(path_ref)) {
                        (Ok(opened_meta), Ok(disk_meta)) => {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::MetadataExt;
                                opened_meta.dev() != disk_meta.dev()
                                    || opened_meta.ino() != disk_meta.ino()
                            }
                            #[cfg(not(unix))]
                            {
                                false
                            }
                        }
                        // 句柄或磁盘元数据获取失败（如底层文件被删除或替换），重新打开
                        _ => true,
                    }
                }
            }
            None => true,
        };

        if need_reopen {
            if let Some(parent) = path_ref.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }

            // 若目标文件已存在但并非当前 V2 WAL 格式，先合并重放旧数据再归并升级为标准 V2 WAL 格式
            if path_ref.exists() {
                if let Ok(mut existing_file) = File::open(path_ref) {
                    let len = existing_file.metadata().map(|m| m.len()).unwrap_or(0);
                    if len > 0 {
                        let mut magic = [0u8; 16];
                        if existing_file.read_exact(&mut magic).is_err() || &magic != WAL_MAGIC {
                            drop(existing_file);
                            drop(guard);
                            if let Ok(old_model) = Self::load_or_replay_wal(path_ref) {
                                self.merge(&old_model);
                            }
                            self.compact_wal(path_ref)?;
                            guard = self
                                .wal_file
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                    }
                }
            }

            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            options.mode(0o600);

            let mut file =
                options
                    .open(path_ref)
                    .map_err(|e| AlgoError::UserDictPersistFailed {
                        path: PathBuf::from(path_ref),
                        source: e,
                    })?;

            // 跨进程锁保护：尝试获取排他锁以防与其他进程初始化交错
            let _ = flock(&file, FlockOperation::LockExclusive);

            // 文件大小为 0 时写入魔数头
            if file.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
                let res = file
                    .write_all(WAL_MAGIC)
                    .map_err(|e| AlgoError::UserDictPersistFailed {
                        path: PathBuf::from(path_ref),
                        source: e,
                    });
                let _ = flock(&file, FlockOperation::Unlock);
                res?;
            } else {
                let _ = flock(&file, FlockOperation::Unlock);
            }

            *guard = Some((PathBuf::from(path_ref), file));
        }

        let py_bytes = pinyin.as_bytes();
        let text_bytes = text.as_bytes();
        if py_bytes.len() > u16::MAX as usize || text_bytes.len() > u16::MAX as usize {
            warn!(
                "待写入的用户词条长度超过 65535 字节上限，跳过写入: py_len={}, text_len={}",
                py_bytes.len(),
                text_bytes.len()
            );
            return Ok(());
        }
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

        // 简易校验和防部分写入损坏（带非零初始种子，杜绝全零块绕过）
        let checksum: u32 = buf
            .iter()
            .fold(WAL_CHECKSUM_SEED, |acc, &b| acc.wrapping_add(b as u32));
        buf.extend_from_slice(&checksum.to_le_bytes());

        if let Some((_, ref mut file)) = *guard {
            // 跨进程锁保护：追加写入期间持有排他锁，防止被并发截断或并发交错写入
            let _ = flock(&*file, FlockOperation::LockExclusive);
            let write_res = file
                .write_all(&buf)
                .map_err(|e| AlgoError::UserDictPersistFailed {
                    path: PathBuf::from(path_ref),
                    source: e,
                });
            let _ = flock(&*file, FlockOperation::Unlock);
            write_res?;
        }

        drop(guard);

        // 同步更新内存模型状态机（遵循 WAL 先写日志后更新内存的设计原则）
        self.record_commit(text, pinyin, timestamp);

        // 运行期累计写入流水计数（解耦打字按键热路径与重度磁盘 IO，由外部在失焦或退出时调用 maybe_compact_wal）
        self.wal_append_counter += 1;

        Ok(())
    }

    /// 检查并在达到压缩阈值时执行 WAL 文件的原子压缩归并 (Compaction)
    ///
    /// # 设计原理
    /// - **实现初衷**：将耗时的全量词条扫描与 `sync_all` 磁盘落盘从用户打字热路径中剥离，
    ///   由外部在输入法失焦（Deactivate）、闲置或退出等安全时机按需触发。
    /// - **核心优势**：保证选词上屏维持微秒级纯页缓存写入，彻底杜绝输入过程中的瞬时停顿。
    pub fn maybe_compact_wal<P: AsRef<Path>>(&mut self, path: P) -> Result<bool, AlgoError> {
        if self.wal_append_counter >= self.compact_threshold {
            self.compact_wal(path)?;
            self.wal_append_counter = 0;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 从二进制缓冲区重放 WAL 记录帧（公共重放逻辑）
    fn replay_wal_records(buf: &[u8], is_v2: bool) -> (Self, bool, usize) {
        let mut model = Self::new();
        let mut record_count = 0;
        let mut cursor = 0;
        let mut has_corrupted_tail = false;
        let header_len = if is_v2 { 16 } else { 12 };

        while cursor + header_len <= buf.len() {
            let record_start = cursor;
            let timestamp =
                u64::from_le_bytes(buf[cursor..cursor + 8].try_into().unwrap_or([0; 8]));
            let count = if is_v2 {
                u32::from_le_bytes(buf[cursor + 8..cursor + 12].try_into().unwrap_or([0; 4]))
            } else {
                1
            };
            let (py_len, text_len) = if is_v2 {
                let p =
                    u16::from_le_bytes(buf[cursor + 12..cursor + 14].try_into().unwrap_or([0; 2]))
                        as usize;
                let t =
                    u16::from_le_bytes(buf[cursor + 14..cursor + 16].try_into().unwrap_or([0; 2]))
                        as usize;
                (p, t)
            } else {
                let p =
                    u16::from_le_bytes(buf[cursor + 8..cursor + 10].try_into().unwrap_or([0; 2]))
                        as usize;
                let t =
                    u16::from_le_bytes(buf[cursor + 10..cursor + 12].try_into().unwrap_or([0; 2]))
                        as usize;
                (p, t)
            };
            cursor += header_len;

            if cursor + py_len + text_len + 4 > buf.len() {
                warn!("[用户词库 WAL] 检测到文件末尾记录截断损坏，安全截断并在后续自动压缩");
                has_corrupted_tail = true;
                break;
            }

            let py_bytes = &buf[cursor..cursor + py_len];
            cursor += py_len;
            let text_bytes = &buf[cursor..cursor + text_len];
            cursor += text_len;

            let expected_checksum =
                u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap_or([0; 4]));
            cursor += 4;

            let payload = &buf[record_start..cursor - 4];
            let actual_checksum: u32 = payload
                .iter()
                .fold(WAL_CHECKSUM_SEED, |acc, &b| acc.wrapping_add(b as u32));
            if actual_checksum != expected_checksum {
                warn!(
                    "[用户词库 WAL] 记录校验和不匹配 (预期: {:#x}, 实际: {:#x})，终止后续失步帧解析并执行归并清理",
                    expected_checksum, actual_checksum
                );
                has_corrupted_tail = true;
                break;
            }

            let py_str = std::str::from_utf8(py_bytes).unwrap_or("");
            let text_str = std::str::from_utf8(text_bytes).unwrap_or("");

            if !py_str.is_empty() && !text_str.is_empty() {
                model.record_commit_count(text_str, py_str, timestamp, count);
                record_count += 1;
            }
        }

        (model, has_corrupted_tail, record_count)
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

        let meta = file.metadata().map_err(|e| AlgoError::DictionaryIo {
            path: PathBuf::from(path_ref),
            source: e,
        })?;
        if meta.len() > crate::user_adaptive::MAX_USER_DICT_BYTES {
            return Err(AlgoError::deserialize_failed(
                path_ref,
                format!(
                    "WAL 用户词库文件体积异常 ({} 字节)，超出 64MB 上限",
                    meta.len()
                ),
            ));
        }

        let mut magic = [0u8; 16];
        if file.read_exact(&mut magic).is_ok() {
            let is_v2 = &magic == WAL_MAGIC;
            let is_v1 = &magic == WAL_MAGIC_V1;
            if is_v2 || is_v1 {
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)
                    .map_err(|e| AlgoError::DictionaryIo {
                        path: PathBuf::from(path_ref),
                        source: e,
                    })?;

                let (model, has_corrupted_tail, record_count) =
                    Self::replay_wal_records(&buf, is_v2);

                if !is_v2
                    || has_corrupted_tail
                    || (record_count >= 50 && record_count > model.total_entries() * 3)
                {
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

        // 若目标原文件存在，先获取跨进程排他锁以防与其他正在追加的进程发生冲突
        let orig_lock_file = if path_ref.exists() {
            if let Ok(f) = OpenOptions::new().read(true).write(true).open(path_ref) {
                let _ = flock(&f, FlockOperation::LockExclusive);
                Some(f)
            } else {
                None
            }
        } else {
            None
        };

        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let pid = std::process::id();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let file_stem = path_ref
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("wal");
        let temp_file_name = format!("{}.{}.{}.tmp", file_stem, pid, timestamp);
        let temp_path = path_ref.with_file_name(temp_file_name);

        struct TempFileGuard<'a>(&'a Path, bool);
        impl<'a> Drop for TempFileGuard<'a> {
            fn drop(&mut self) {
                if !self.1 {
                    let _ = std::fs::remove_file(self.0);
                }
            }
        }

        let mut guard = TempFileGuard(&temp_path, false);

        {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            options.mode(0o600);

            let mut file =
                options
                    .open(&temp_path)
                    .map_err(|e| AlgoError::UserDictPersistFailed {
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
                    if py_bytes.len() > u16::MAX as usize || text_bytes.len() > u16::MAX as usize {
                        continue;
                    }
                    let py_len = py_bytes.len() as u16;
                    let text_len = text_bytes.len() as u16;

                    let mut buf = Vec::with_capacity(20 + py_bytes.len() + text_bytes.len());
                    buf.extend_from_slice(&entry.last_used_timestamp.to_le_bytes());
                    buf.extend_from_slice(&entry.commit_count.to_le_bytes());
                    buf.extend_from_slice(&py_len.to_le_bytes());
                    buf.extend_from_slice(&text_len.to_le_bytes());
                    buf.extend_from_slice(py_bytes);
                    buf.extend_from_slice(text_bytes);
                    let checksum: u32 = buf
                        .iter()
                        .fold(WAL_CHECKSUM_SEED, |acc, &b| acc.wrapping_add(b as u32));
                    buf.extend_from_slice(&checksum.to_le_bytes());

                    file.write_all(&buf)
                        .map_err(|e| AlgoError::UserDictPersistFailed {
                            path: temp_path.clone(),
                            source: e,
                        })?;
                }
            }

            file.sync_all()
                .map_err(|e| AlgoError::UserDictPersistFailed {
                    path: temp_path.clone(),
                    source: e,
                })?;
        }

        // 在原子重命名前即刻激活内部归并标记，防止内核派发重命名/删除事件与落盘标记产生竞态
        self.mark_internal_compact();

        std::fs::rename(&temp_path, path_ref).map_err(|e| AlgoError::UserDictPersistFailed {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        // 成功完成原子重命名后解除守卫删除并刷新内部落盘完成时间戳
        guard.1 = true;
        self.mark_internal_compact();

        if let Some(f) = orig_lock_file {
            let _ = flock(&f, FlockOperation::Unlock);
        }

        Ok(())
    }
}
