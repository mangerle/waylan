//! 本地模型物理导入、文件复制与用户模型存储目录管理
//!
//! # 设计原理
//! - **实现初衷**：为用户导入本地离线 ASR 模型包或单个 ONNX 权重提供安全的物理复制与目录维护能力。
//! - **核心优势**：严格遵循 Linux XDG 规范，实现系统内置目录与用户导入目录物理隔离；子进程轻量收割防僵尸进程。
//! - **代价与局限**：受目标磁盘可用空间与文件系统权限约束。

use std::ffi::OsStr;
use std::fs;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use log::{info, warn};

use super::ModelSubdirectory;

/// 获取用户个人模型存储根目录 (`~/.local/share/waylan/models/`)
pub fn user_models_dir() -> Option<PathBuf> {
    waylan_core::config::user_models_dir()
}

/// 确保用户模型目录及其四个类型子目录全部创建就绪
pub fn ensure_user_models_dirs() -> Option<PathBuf> {
    let base = user_models_dir()?;
    let subdirs = ["vad", "asr", "ns"];
    for sub in &subdirs {
        let p = base.join(sub);
        if !p.exists() {
            let _ = fs::create_dir_all(&p);
        }
    }
    Some(base)
}

/// 在系统文件管理器中打开用户模型存储目录（通过 xdg-open），并异步收割子进程以防僵尸进程残留
pub fn open_user_models_directory() -> bool {
    if let Some(dir) = ensure_user_models_dirs() {
        info!("[模型管理] 正在系统文件管理器中打开用户模型目录: {:?}", dir);
        match Command::new("xdg-open")
            .arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // 派发轻量收割线程等待子进程退出并释放进程控制块 (PCB)，杜绝僵尸进程残留
                if let Err(e) = std::thread::Builder::new()
                    .name("xdg-open-reaper".to_string())
                    .spawn(move || {
                        let _ = child.wait();
                    })
                {
                    warn!("[模型管理 警告] 派发子进程收割线程失败: {}", e);
                }
                true
            }
            Err(e) => {
                warn!("[模型管理 警告] 调用 xdg-open 打开目录失败: {}", e);
                false
            }
        }
    } else {
        false
    }
}

/// 获取系统官方内置模型根目录（deb 安装路径 `/usr/share/waylan/models` 或源码工作区路径 `./models`）
pub fn builtin_models_dir() -> Option<PathBuf> {
    let sys_dir = PathBuf::from("/usr/share/waylan/models");
    if sys_dir.is_dir() {
        return Some(sys_dir);
    }
    let dev_dir = PathBuf::from("models");
    if dev_dir.is_dir() {
        return Some(dev_dir);
    }
    None
}

/// 目录递归复制最大深度上限，防止异常深层目录触发栈溢出
const MAX_COPY_DEPTH: usize = 16;

/// 全局单调暂存序列号，防止并发导入同名模型时临时路径冲突
static IMPORT_SEQ: AtomicU64 = AtomicU64::new(1);

/// 递归复制目录内容（严格拒绝符号链接与特殊文件，并限制目录嵌套深度）
///
/// # Errors
/// 当包含符号链接、非普通文件、嵌套深度超出上限或底层文件读写失败时返回 IO 错误。
pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    copy_dir_all_bounded(src, dst, 0)
}

fn copy_dir_all_bounded(src: &Path, dst: &Path, depth: usize) -> std::io::Result<()> {
    if depth > MAX_COPY_DEPTH {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("模型目录层级超过最大限制 ({MAX_COPY_DEPTH} 层)"),
        ));
    }
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let entry_path = entry.path();
        if ty.is_symlink() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("拒绝复制符号链接条目: {:?}", entry_path),
            ));
        }
        let target_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all_bounded(&entry_path, &target_path, depth + 1)?;
        } else if ty.is_file() {
            fs::copy(&entry_path, &target_path)?;
        } else {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("拒绝复制非普通文件条目: {:?}", entry_path),
            ));
        }
    }
    Ok(())
}

/// 生成同父目录下的唯一临时暂存路径
fn build_unique_staging_path(parent: &Path, prefix: &str, name: &OsStr) -> PathBuf {
    let seq = IMPORT_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let name_lossy = name.to_string_lossy();
    parent.join(format!(".{prefix}_{pid}_{seq}_{name_lossy}"))
}

/// 校验待导入源路径与目标路径不重名、不互相包含且源路径非符号链接逃逸
fn validate_import_paths(src_path: &Path, target_parent: &Path, target_name: &OsStr) -> bool {
    let Ok(canon_src) = fs::canonicalize(src_path) else {
        return false;
    };
    let Ok(canon_parent) = fs::canonicalize(target_parent) else {
        return false;
    };
    let canon_target = canon_parent.join(target_name);
    if canon_src == canon_target || canon_target.starts_with(&canon_src) {
        warn!(
            "[模型管理] 拒绝导入指向自身或子路径的目录/文件: src={:?}, target={:?}",
            canon_src, canon_target
        );
        return false;
    }
    true
}

/// 在同父目录内通过临时目录与可选备份完成原子目录替换
fn atomic_replace_dir(staging_dir: &Path, target_dir: &Path) -> std::io::Result<()> {
    let mut backup_dir: Option<PathBuf> = None;
    if target_dir.exists() {
        let Some(parent) = target_dir.parent() else {
            return Err(Error::new(ErrorKind::InvalidInput, "目标目录缺少父目录"));
        };
        let Some(name) = target_dir.file_name() else {
            return Err(Error::new(ErrorKind::InvalidInput, "目标目录缺少目录名"));
        };
        let bak = build_unique_staging_path(parent, "waylan_import_bak", name);
        fs::rename(target_dir, &bak)?;
        backup_dir = Some(bak);
    }

    if let Err(err) = fs::rename(staging_dir, target_dir) {
        if let Some(bak) = backup_dir {
            let _ = fs::rename(bak, target_dir);
        }
        return Err(err);
    }

    if let Some(bak) = backup_dir {
        let _ = fs::remove_dir_all(bak);
    }
    Ok(())
}

/// 将指定的外部 ASR 复合模型文件夹导入至指定用户模型根目录（先暂存复制，再原子替换）
fn import_asr_model_to_base(src_path: &Path, user_base: &Path) -> Option<PathBuf> {
    if !src_path.is_dir() {
        return None;
    }
    let dir_name = src_path.file_name()?;
    let asr_parent = user_base.join("asr");
    if fs::create_dir_all(&asr_parent).is_err() {
        return None;
    }
    if !validate_import_paths(src_path, &asr_parent, dir_name) {
        return None;
    }

    let target_dir = asr_parent.join(dir_name);
    let staging_dir = build_unique_staging_path(&asr_parent, "waylan_import_tmp", dir_name);
    info!(
        "[模型管理] 正在将 ASR 模型从 {:?} 导入至 {:?}",
        src_path, target_dir
    );
    if let Err(e) = copy_dir_all(src_path, &staging_dir) {
        warn!("[模型管理] 复制 ASR 模型目录失败: {}", e);
        let _ = fs::remove_dir_all(&staging_dir);
        return None;
    }
    if let Err(e) = atomic_replace_dir(&staging_dir, &target_dir) {
        warn!("[模型管理] 原子切换 ASR 模型目录失败: {}", e);
        let _ = fs::remove_dir_all(&staging_dir);
        return None;
    }
    Some(target_dir)
}

/// 将指定的外部 ASR 复合模型文件夹导入并拷贝至用户模型目录
pub fn import_asr_model_from_dir(src_path: &Path) -> Option<PathBuf> {
    let base = ensure_user_models_dirs()?;
    import_asr_model_to_base(src_path, &base)
}

/// 将指定的外部单个 ONNX 模型文件导入至指定用户模型根目录下的受控子目录
fn import_onnx_model_to_base(
    subdir: ModelSubdirectory,
    src_path: &Path,
    user_base: &Path,
) -> Option<PathBuf> {
    let meta = fs::symlink_metadata(src_path).ok()?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        warn!("[模型管理] 拒绝导入符号链接或非普通文件: {:?}", src_path);
        return None;
    }
    let file_name = src_path.file_name()?;
    let target_parent = user_base.join(subdir.as_str());
    if fs::create_dir_all(&target_parent).is_err() {
        return None;
    }
    if !validate_import_paths(src_path, &target_parent, file_name) {
        return None;
    }

    let target_file = target_parent.join(file_name);
    let staging_file = build_unique_staging_path(&target_parent, "waylan_import_tmp", file_name);
    info!(
        "[模型管理] 正在将模型文件从 {:?} 导入至 {:?}",
        src_path, target_file
    );
    if let Err(e) = fs::copy(src_path, &staging_file) {
        warn!("[模型管理] 复制模型文件失败: {}", e);
        let _ = fs::remove_file(&staging_file);
        return None;
    }
    if let Err(e) = fs::rename(&staging_file, &target_file) {
        warn!("[模型管理] 原子切换模型文件失败: {}", e);
        let _ = fs::remove_file(&staging_file);
        return None;
    }
    Some(target_file)
}

/// 将指定的外部单个 ONNX 模型文件导入至用户模型的指定受控子目录
pub fn import_onnx_model_from_file(subdir: ModelSubdirectory, src_path: &Path) -> Option<PathBuf> {
    let base = ensure_user_models_dirs()?;
    import_onnx_model_to_base(subdir, src_path, &base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn test_import_asr_dir_rejects_symlinks_and_preserves_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("user_models");
        let existing = base.join("asr").join("my_model");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("keep.txt"), b"old_valid").unwrap();

        let src = tmp.path().join("src").join("my_model");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("model.onnx"), b"new_data").unwrap();
        let secret = tmp.path().join("secret.txt");
        fs::write(&secret, b"secret").unwrap();
        symlink(&secret, src.join("leak.txt")).unwrap();

        assert!(import_asr_model_to_base(&src, &base).is_none());
        assert_eq!(fs::read(existing.join("keep.txt")).unwrap(), b"old_valid");
    }

    #[test]
    fn test_import_asr_dir_blocks_self_and_ancestor_recursion() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("user_models");
        let self_dir = base.join("asr").join("same_model");
        fs::create_dir_all(&self_dir).unwrap();
        fs::write(self_dir.join("tokens.txt"), b"ok").unwrap();

        assert!(import_asr_model_to_base(&self_dir, &base).is_none());
        assert!(import_asr_model_to_base(tmp.path(), &base).is_none());
    }

    #[test]
    fn test_import_asr_and_onnx_atomic_success() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("user_models");
        let src_dir = tmp.path().join("custom_asr");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("tokens.txt"), b"vocab").unwrap();

        let imported_dir = import_asr_model_to_base(&src_dir, &base).unwrap();
        assert_eq!(fs::read(imported_dir.join("tokens.txt")).unwrap(), b"vocab");

        let onnx_src = tmp.path().join("vad.onnx");
        fs::write(&onnx_src, b"weights").unwrap();
        let imported_onnx =
            import_onnx_model_to_base(ModelSubdirectory::Vad, &onnx_src, &base).unwrap();
        assert_eq!(fs::read(imported_onnx).unwrap(), b"weights");

        let link_src = tmp.path().join("link_vad.onnx");
        symlink(&onnx_src, &link_src).unwrap();
        assert!(import_onnx_model_to_base(ModelSubdirectory::Vad, &link_src, &base).is_none());
    }
}
