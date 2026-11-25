//! 模型 `.tar.bz2` 归档安全流式解压与沙箱路径校验模块
//!
//! # 设计原理
//! - **实现初衷**：替代外部 `tar` 子进程及不安全的纯路径文本校验（无法识别符号链接绕过），
//!   以纯 Rust 流式解析归档条目头与内容流，实现编译期可控的解压沙箱。
//! - **核心优势**：
//!   1. 严格白名单放行 `Regular` 与 `Directory`，彻底拒绝 `Symlink`、`Hardlink` 及特殊设备节点；
//!   2. 逐条目校验路径分量并结合 `canonicalize` 父目录前缀检查，彻底根除 Tar Slip 路径穿越；
//!   3. 累计解压字节上限防御压缩炸弹，消除 `thread::sleep` 子进程轮询。
//! - **代价与局限**：仅支持标准 `.tar.bz2` 压缩归档。

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use bzip2::read::BzDecoder;
use tar::{Archive, Entry, EntryType};

use crate::error::VoiceError;

/// 模型归档解压后最大允许总字节数（4 GiB），防范压缩炸弹
const MAX_MODEL_EXTRACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// 校验归档相对路径安全并解析受控落盘路径（严防绝对路径、`..` 与软链接逃逸）
fn resolve_safe_extract_path(
    canonical_dest: &Path,
    rel_path: &Path,
) -> Result<PathBuf, VoiceError> {
    let mut cleaned = PathBuf::new();
    for comp in rel_path.components() {
        match comp {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(VoiceError::ModelManagementFailed {
                    message: format!("模型归档包含不安全的越权路径条目: {}", rel_path.display()),
                });
            }
        }
    }

    if cleaned.as_os_str().is_empty() {
        return Ok(canonical_dest.to_path_buf());
    }

    let target = canonical_dest.join(&cleaned);
    if let Some(parent) = target.parent()
        && parent.exists()
    {
        let canonical_parent =
            fs::canonicalize(parent).map_err(|e| VoiceError::ModelManagementFailed {
                message: format!("校验解压目标父目录失败: {}", e),
            })?;
        if !canonical_parent.starts_with(canonical_dest) {
            return Err(VoiceError::ModelManagementFailed {
                message: format!("模型归档条目路径逃逸出沙箱目录: {}", rel_path.display()),
            });
        }
    }
    Ok(target)
}

/// 校验并解压单个 tar 条目至沙箱目录，返回本次写出的字节数
fn extract_single_entry<R: std::io::Read>(
    entry: &mut Entry<'_, R>,
    canonical_dest: &Path,
    total_extracted: &mut u64,
) -> Result<(), VoiceError> {
    let entry_type = entry.header().entry_type();
    if !matches!(entry_type, EntryType::Regular | EntryType::Directory) {
        return Err(VoiceError::ModelManagementFailed {
            message: format!(
                "模型归档包含禁止的非普通文件条目类型 ({:?})，已拒绝解压",
                entry_type
            ),
        });
    }

    let rel_path = entry
        .path()
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("读取归档条目路径失败: {}", e),
        })?
        .into_owned();
    let target_path = resolve_safe_extract_path(canonical_dest, &rel_path)?;

    if entry_type == EntryType::Directory {
        return fs::create_dir_all(&target_path).map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("创建解压目录失败: {}", e),
        });
    }

    let entry_size = entry.size();
    *total_extracted = total_extracted.saturating_add(entry_size);
    if *total_extracted > MAX_MODEL_EXTRACT_BYTES {
        return Err(VoiceError::ModelManagementFailed {
            message: format!(
                "模型归档解压总体积超过安全上限 ({} 字节)，已中止解压",
                MAX_MODEL_EXTRACT_BYTES
            ),
        });
    }

    write_regular_entry_file(entry, &target_path, entry_size)
}

/// 将普通归档条目写入目标文件并核对写出字节数
fn write_regular_entry_file<R: std::io::Read>(
    entry: &mut Entry<'_, R>,
    target_path: &Path,
    expected_size: u64,
) -> Result<(), VoiceError> {
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("创建解压父目录失败: {}", e),
        })?;
    }
    let mut out_file =
        File::create(target_path).map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("创建解压目标文件失败: {}", e),
        })?;
    let copied =
        std::io::copy(entry, &mut out_file).map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("写出解压文件失败: {}", e),
        })?;
    if copied > expected_size {
        return Err(VoiceError::ModelManagementFailed {
            message: "解压条目实际字节数与头部声明不符".to_string(),
        });
    }
    Ok(())
}

/// 纯 Rust 安全流式解压 `.tar.bz2` 归档（禁止软硬链接/设备节点，防路径穿越与压缩炸弹）
///
/// # Errors
/// 当归档打开失败、包含非法条目类型或越权路径、或解压体积超过上限时返回错误。
pub(crate) fn extract_tar_bz2_safely(
    archive_path: &Path,
    dest_dir: &Path,
) -> Result<(), VoiceError> {
    let canonical_dest =
        fs::canonicalize(dest_dir).map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("规范化解压目标目录失败: {}", e),
        })?;
    let file = File::open(archive_path).map_err(|e| VoiceError::ModelManagementFailed {
        message: format!("打开模型归档文件失败: {}", e),
    })?;
    let mut archive = Archive::new(BzDecoder::new(BufReader::new(file)));
    let mut total_extracted: u64 = 0;

    let entries = archive
        .entries()
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("读取模型归档条目失败，可能文件已损坏: {}", e),
        })?;

    for entry_res in entries {
        let mut entry = entry_res.map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("解析模型归档条目失败: {}", e),
        })?;
        extract_single_entry(&mut entry, &canonical_dest, &mut total_extracted)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bzip2::Compression;
    use bzip2::write::BzEncoder;

    fn build_tar_bz2(
        build_fn: impl FnOnce(&mut tar::Builder<BzEncoder<File>>),
    ) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("创建临时测试目录失败");
        let archive_path = dir.path().join("test.tar.bz2");
        let file = File::create(&archive_path).expect("创建测试压缩包失败");
        let encoder = BzEncoder::new(file, Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        build_fn(&mut builder);
        let encoder = builder.into_inner().expect("完成 tar 封装失败");
        encoder.finish().expect("完成 bzip2 压缩失败");
        (dir, archive_path)
    }

    #[test]
    fn test_extract_rejects_symlink_tar_slip() {
        let (_src_dir, archive_path) = build_tar_bz2(|builder| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder
                .append_link(&mut header, "sub_link", "/tmp")
                .expect("写入软链接条目失败");
        });

        let dest = tempfile::tempdir().expect("创建解压目录失败");
        let res = extract_tar_bz2_safely(&archive_path, dest.path());
        assert!(res.is_err(), "包含软链接条目的归档必须被拒绝");
    }

    #[test]
    fn test_extract_rejects_hardlink_tar_slip() {
        let (_src_dir, archive_path) = build_tar_bz2(|builder| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Link);
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_link(&mut header, "hard_link", "/etc/passwd")
                .expect("写入硬链接条目失败");
        });

        let dest = tempfile::tempdir().expect("创建解压目录失败");
        let res = extract_tar_bz2_safely(&archive_path, dest.path());
        assert!(res.is_err(), "包含硬链接条目的归档必须被拒绝");
    }

    #[test]
    fn test_resolve_safe_extract_path_blocks_traversal() {
        let dest = tempfile::tempdir().expect("创建沙箱目录失败");
        let canonical = fs::canonicalize(dest.path()).expect("规范化目录失败");
        assert!(resolve_safe_extract_path(&canonical, Path::new("../escape.txt")).is_err());
        assert!(resolve_safe_extract_path(&canonical, Path::new("sub/../../escape.txt")).is_err());
        assert!(resolve_safe_extract_path(&canonical, Path::new("/abs/escape.txt")).is_err());
        let ok_path = resolve_safe_extract_path(&canonical, Path::new("./a/b.txt"))
            .expect("合法相对路径应通过");
        assert!(ok_path.starts_with(&canonical));
    }

    #[test]
    fn test_extract_accepts_regular_files_within_sandbox() {
        let (_src_dir, archive_path) = build_tar_bz2(|builder| {
            let data = b"sensevoice-weights";
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "sensevoice/tokens.txt", &data[..])
                .expect("写入普通文件条目失败");
        });

        let dest = tempfile::tempdir().expect("创建解压目录失败");
        extract_tar_bz2_safely(&archive_path, dest.path()).expect("合法归档应解压成功");
        assert_eq!(
            fs::read(dest.path().join("sensevoice/tokens.txt")).expect("读取解压文件失败"),
            b"sensevoice-weights"
        );
    }
}
