//! 本地语音模型扫描发现、用户目录物理隔离与外部导入管理模块
//!
//! # 设计原理
//! - **实现初衷**：为输入法提供本地模型统一的发现扫描、类型分类与无损导入能力。
//! - **核心优势**：严格遵循 Linux XDG 规范，实现系统内置目录 (`/usr/share/waylan/models/`) 与用户
//!   个性化导入目录 (`~/.local/share/waylan/models/`) 的物理权限隔离，确保系统升级重装绝不损坏用户导入模型。
//! - **代价与局限**：图形化导入调用桌面标准的 `zenity` 原生对话框工具，若在纯 CLI 环境将自动优雅降级。

use log::{info, warn};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 可供用户选择的模型项描述
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelItem {
    /// 界面友好展示名称
    pub display_name: String,
    /// 对应的配置文件存储路径；为 None 时代表自动使用系统内置或默认模型
    pub path: Option<String>,
}

/// 获取用户个人模型存储根目录 (`~/.local/share/waylan/models/`)
pub fn user_models_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".local/share/waylan/models"))
}

/// 确保用户模型目录及其三个类型子目录全部创建就绪
pub fn ensure_user_models_dirs() -> Option<PathBuf> {
    let base = user_models_dir()?;
    let subdirs = ["vad", "asr", "punct"];
    for sub in &subdirs {
        let p = base.join(sub);
        if !p.exists() {
            let _ = fs::create_dir_all(&p);
        }
    }
    Some(base)
}

/// 在系统文件管理器中打开用户模型存储目录（通过 xdg-open）
pub fn open_user_models_directory() -> bool {
    if let Some(dir) = ensure_user_models_dirs() {
        info!("[模型管理] 正在系统文件管理器中打开用户模型目录: {:?}", dir);
        Command::new("xdg-open").arg(&dir).spawn().is_ok()
    } else {
        false
    }
}

/// 扫描并列出所有可用的 ASR 识别模型（内置系统模型 + 用户导入的子文件夹）
pub fn list_asr_models() -> Vec<ModelItem> {
    let mut items = vec![ModelItem {
        display_name: "[内置] 阿里 Paraformer (中文流式 int8)".to_string(),
        path: None,
    }];

    // 扫描用户自定义目录: ~/.local/share/waylan/models/asr/*/
    if let Some(user_dir) = user_models_dir() {
        let asr_dir = user_dir.join("asr");
        if let Ok(entries) = fs::read_dir(asr_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let dir_name = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if dir_name == "paraformer-zh" {
                        continue; // 与内置同名，保留内置选项即可
                    }
                    items.push(ModelItem {
                        display_name: format!("[用户导入] {}", dir_name),
                        path: Some(p.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }

    items
}

/// 扫描并列出所有可用的 VAD 端点检测模型/算法
pub fn list_vad_models() -> Vec<ModelItem> {
    let mut items = vec![
        ModelItem {
            display_name: "[内置] Silero VAD (神经网络模型)".to_string(),
            path: None,
        },
        ModelItem {
            display_name: "[算法] 纯能量门限 (零模型纯数学)".to_string(),
            path: Some("__energy__".to_string()),
        },
    ];

    // 扫描用户自定义目录: ~/.local/share/waylan/models/vad/*.onnx
    if let Some(user_dir) = user_models_dir() {
        let vad_dir = user_dir.join("vad");
        if let Ok(entries) = fs::read_dir(vad_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().map(|e| e == "onnx").unwrap_or(false) {
                    let file_name = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if file_name == "silero_vad.onnx" {
                        continue;
                    }
                    items.push(ModelItem {
                        display_name: format!("[用户导入] {}", file_name),
                        path: Some(p.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }

    items
}

/// 扫描并列出所有可用的标点符号预测模型
pub fn list_punct_models() -> Vec<ModelItem> {
    let mut items = vec![
        ModelItem {
            display_name: "[内置] CT-Transformer 深度标点预测".to_string(),
            path: None,
        },
        ModelItem {
            display_name: "[规则] 基础句末规则兜底 (无模型)".to_string(),
            path: Some("__rule_only__".to_string()),
        },
    ];

    // 扫描用户自定义目录: ~/.local/share/waylan/models/punct/*.onnx
    if let Some(user_dir) = user_models_dir() {
        let punct_dir = user_dir.join("punct");
        if let Ok(entries) = fs::read_dir(punct_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().map(|e| e == "onnx").unwrap_or(false) {
                    let file_name = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if file_name == "model.onnx" {
                        continue;
                    }
                    items.push(ModelItem {
                        display_name: format!("[用户导入] {}", file_name),
                        path: Some(p.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }

    items
}

/// 递归复制目录内容
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
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

/// 调用系统原生对话框 (zenity) 导入 ASR 复合模型文件夹并拷贝至用户目录
pub fn import_asr_model_dialog() -> Option<PathBuf> {
    ensure_user_models_dirs();
    let output = Command::new("zenity")
        .arg("--file-selection")
        .arg("--directory")
        .arg("--title=请选择包含 ASR 模型的目录 (需包含 encoder/decoder/tokens)")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let selected_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_str.is_empty() {
        return None;
    }

    let src_path = PathBuf::from(&selected_str);
    if !src_path.is_dir() {
        return None;
    }

    let dir_name = src_path.file_name()?;
    let target_dir = user_models_dir()?.join("asr").join(dir_name);

    info!(
        "[模型管理] 正在将 ASR 模型从 {:?} 导入至 {:?}",
        src_path, target_dir
    );
    if let Err(e) = copy_dir_all(&src_path, &target_dir) {
        warn!("[模型管理] 复制 ASR 模型目录失败: {}", e);
        return None;
    }

    Some(target_dir)
}

/// 调用系统原生对话框 (zenity) 导入单个 ONNX 模型文件至指定子目录 (vad 或 punct)
pub fn import_onnx_model_dialog(subdir: &str, title: &str) -> Option<PathBuf> {
    ensure_user_models_dirs();
    let output = Command::new("zenity")
        .arg("--file-selection")
        .arg("--file-filter=ONNX 神经网络模型 (*.onnx) | *.onnx")
        .arg(format!("--title={}", title))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let selected_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_str.is_empty() {
        return None;
    }

    let src_path = PathBuf::from(&selected_str);
    if !src_path.is_file() {
        return None;
    }

    let file_name = src_path.file_name()?;
    let target_file = user_models_dir()?.join(subdir).join(file_name);

    info!(
        "[模型管理] 正在将模型文件从 {:?} 导入至 {:?}",
        src_path, target_file
    );
    if let Err(e) = fs::copy(&src_path, &target_file) {
        warn!("[模型管理] 复制模型文件失败: {}", e);
        return None;
    }

    Some(target_file)
}
