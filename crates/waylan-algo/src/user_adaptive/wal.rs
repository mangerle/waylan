use crate::error::AlgoError;
use crate::user_adaptive::UserAdaptiveModel;
use log::warn;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 旧版 WAL 预写日志魔数标识（16 字节，V1 记录未携带频次字段）
pub const WAL_MAGIC_V1: &[u8; 16] = b"WAYLAN_USERWAL01";

/// 当前 WAL 预写日志魔数标识（16 字节，V2 记录原生支持 4 字节频次字段）
pub const WAL_MAGIC: &[u8; 16] = b"WAYLAN_USERWAL02";

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

        // 同步更新内存模型状态机（遵循 WAL 先写日志后更新内存的设计原则）
        self.record_commit(text, pinyin, timestamp);

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
                let mut has_corrupted_tail = false;
                while cursor + 16 <= buf.len() {
                    let record_start = cursor;
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
                        warn!(
                            "[用户词库 WAL] 检测到文件末尾记录截断损坏，安全截断并在后续自动压缩"
                        );
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

                    // 验证该记录前缀所有字节的累加校验和
                    let payload = &buf[record_start..cursor - 4];
                    let actual_checksum: u32 = payload
                        .iter()
                        .fold(0u32, |acc, &b| acc.wrapping_add(b as u32));
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

                // 启动加载自动压缩判定：
                // 若存在尾部截断/校验损坏，或者记录流水数 >= 50 且超过独立有效词条数的 3 倍，自动触发一次原子压缩
                if has_corrupted_tail
                    || (record_count >= 50 && record_count > model.total_entries() * 3)
                {
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
                let mut has_corrupted_tail = false;
                while cursor + 12 <= buf.len() {
                    let record_start = cursor;
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
                        warn!("[用户词库 WAL V1] 检测到文件末尾记录截断损坏，安全截断并在后续自动压缩");
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
                        .fold(0u32, |acc, &b| acc.wrapping_add(b as u32));
                    if actual_checksum != expected_checksum {
                        warn!(
                            "[用户词库 WAL V1] 记录校验和不匹配，终止后续失步帧解析并执行归并清理"
                        );
                        has_corrupted_tail = true;
                        break;
                    }

                    let py_str = std::str::from_utf8(py_bytes).unwrap_or("");
                    let text_str = std::str::from_utf8(text_bytes).unwrap_or("");

                    if !py_str.is_empty() && !text_str.is_empty() {
                        model.record_commit(text_str, py_str, timestamp);
                        record_count += 1;
                    }
                }

                if has_corrupted_tail
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

        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let temp_path = path_ref.with_extension("wal.tmp");

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
                    let checksum: u32 = buf.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32));
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

        std::fs::rename(&temp_path, path_ref).map_err(|e| AlgoError::UserDictPersistFailed {
            path: PathBuf::from(path_ref),
            source: e,
        })?;

        // 成功完成原子重命名后解除守卫删除
        guard.1 = true;

        Ok(())
    }
}
