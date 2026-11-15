//! 本地模型物理导入、文件复制与用户模型存储目录管理
//!
//! # 设计原理
//! - **实现初衷**：为用户导入本地离线 ASR 模型包或单个 ONNX 权重提供安全的物理复制与目录维护能力。
//! - **核心优势**：严格遵循 Linux XDG 规范，实现系统内置目录与用户导入目录物理隔离；子进程轻量收割防僵尸进程。
//! - **代价与局限**：受目标磁盘可用空间与文件系统权限约束。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use log::{info, warn};

use super::ModelSubdirectory;

/// 获取用户个人模型存储根目录 (`~/.local/share/waylan/models/`)
pub fn user_models_dir() -> Option<PathBuf> {
    waylan_core::config::user_models_dir()
}

/// 确保用户模型目录及其四个类型子目录全部创建就绪
pub fn ensure_user_models_dirs() -> Option<PathBuf> {
    let base = user_models_dir()?;
    let subdirs = ["vad", "asr", "punct", "ns"];
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

/// 递归复制目录内容
pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// 将指定的外部 ASR 复合模型文件夹导入并拷贝至用户模型目录
pub fn import_asr_model_from_dir(src_path: &Path) -> Option<PathBuf> {
    ensure_user_models_dirs();
    if !src_path.is_dir() {
        return None;
    }

    let dir_name = src_path.file_name()?;
    let target_dir = user_models_dir()?.join("asr").join(dir_name);

    info!(
        "[模型管理] 正在将 ASR 模型从 {:?} 导入至 {:?}",
        src_path, target_dir
    );
    if let Err(e) = copy_dir_all(src_path, &target_dir) {
        warn!("[模型管理] 复制 ASR 模型目录失败: {}", e);
        return None;
    }

    Some(target_dir)
}

/// 将指定的外部单个 ONNX 模型文件导入至用户模型的指定受控子目录
pub fn import_onnx_model_from_file(subdir: ModelSubdirectory, src_path: &Path) -> Option<PathBuf> {
    ensure_user_models_dirs();
    if !src_path.is_file() {
        return None;
    }

    let file_name = src_path.file_name()?;
    let target_file = user_models_dir()?.join(subdir.as_str()).join(file_name);

    info!(
        "[模型管理] 正在将模型文件从 {:?} 导入至 {:?}",
        src_path, target_file
    );
    if let Err(e) = fs::copy(src_path, &target_file) {
        warn!("[模型管理] 复制模型文件失败: {}", e);
        return None;
    }

    Some(target_file)
}
