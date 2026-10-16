//! WAL 预写日志底层常数与进程内落盘同步状态
//!
//! # 设计原理
//! - **实现初衷**：集中管理预写日志魔数标识、校验和计算种子及全进程最近落盘时间戳，
//!   杜绝魔数散落，并在发生磁盘落盘时提供毫秒级时间窗防重机制。
//! - **核心优势**：常量与轻量原子状态高内聚，零依赖业务逻辑。

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
