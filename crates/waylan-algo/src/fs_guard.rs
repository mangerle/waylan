//! 本地文件读取前的体积安全守卫
//!
//! # 设计原理
//! - **实现初衷**：算法层多处需要把磁盘词库与用户数据文件整体读入内存解析，
//!   一旦文件损坏或被恶意/异常膨胀，`read_to_end` 与 `read_to_string` 会在做任何格式校验之前
//!   先把全部字节搬进堆，直接触发 OOM 或长时间停顿。
//! - **核心优势**：在打开文件前后、真正读取之前用 `metadata` 做一次上限比对，
//!   以极低代价把"未知体量"收敛为"已知有界"，让上层可以放心整体读入。
//! - **代价与局限**：`metadata` 与实际读取之间存在竞态窗口（TOCTOU），若外部进程在两者之间
//!   继续追加写入，仍可能读到超限内容；本模块仅覆盖输入法自身与用户手工编辑的静态数据文件，
//!   不用于并发写入的日志流。

use std::path::{Path, PathBuf};

use crate::error::AlgoError;

/// 校验文件体积是否处于安全上限内，超限返回 [`AlgoError::FileTooLarge`]
///
/// # Errors
/// 当无法读取文件元数据时返回 [`AlgoError::DictionaryIo`]，
/// 当文件体积超过 `limit_bytes` 时返回 [`AlgoError::FileTooLarge`]。
pub(crate) fn ensure_within_limit(path: &Path, limit_bytes: u64) -> Result<(), AlgoError> {
    let actual_bytes =
        path.metadata()
            .map(|meta| meta.len())
            .map_err(|e| AlgoError::DictionaryIo {
                path: PathBuf::from(path),
                source: e,
            })?;

    if actual_bytes > limit_bytes {
        return Err(AlgoError::FileTooLarge {
            path: PathBuf::from(path),
            actual_bytes,
            limit_bytes,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_file_over_limit() {
        let dir = std::env::temp_dir().join(format!("waylan_fs_guard_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("oversize.bin");
        std::fs::write(&file, vec![0u8; 128]).expect("写入测试文件失败");

        let err = ensure_within_limit(&file, 64).expect_err("超限文件必须被拒绝");
        assert!(matches!(
            err,
            AlgoError::FileTooLarge {
                actual_bytes: 128,
                limit_bytes: 64,
                ..
            }
        ));
        assert!(err.to_string().contains("词库文件体积超过安全上限"));

        ensure_within_limit(&file, 1024).expect("未超限文件必须放行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_missing_file_returns_io_error() {
        let missing = Path::new("/nonexistent/waylan_fs_guard_missing.bin");
        let err = ensure_within_limit(missing, 1024).expect_err("缺失文件必须报错");
        assert!(matches!(err, AlgoError::DictionaryIo { .. }));
    }
}
