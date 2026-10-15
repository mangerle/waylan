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
    let subdirs = ["vad", "asr", "punct", "ns"];
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

/// 扫描并列出所有可用的 ASR 识别模型（动态扫描系统内置目录与用户导入目录）
pub fn list_asr_models() -> Vec<ModelItem> {
    let mut items = Vec::new();

    // 1. 动态扫描系统内置目录: /usr/share/waylan/models/asr/ 或 models/asr/
    if let Some(builtin_dir) = builtin_models_dir() {
        let asr_dir = builtin_dir.join("asr");
        if let Ok(entries) = fs::read_dir(asr_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let dir_name = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    items.push(ModelItem {
                        display_name: format!("[内置] {}", dir_name),
                        path: Some(p.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }

    // 优先将包含 zipformer 的模型排在推荐首位
    items.sort_by(|a, b| {
        let a_is_zip = a.display_name.to_lowercase().contains("zipformer");
        let b_is_zip = b.display_name.to_lowercase().contains("zipformer");
        b_is_zip.cmp(&a_is_zip)
    });

    // 若系统目录未扫描到任何内置 ASR 模型，提供默认项保底
    if items.is_empty() {
        items.push(ModelItem {
            display_name: "[内置] zipformer-zh".to_string(),
            path: None,
        });
    }

    // 2. 动态扫描用户私有目录: ~/.local/share/waylan/models/asr/*/
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
                    if items
                        .iter()
                        .any(|m| m.path.as_deref() == Some(&p.to_string_lossy()))
                        || dir_name == "zipformer-zh"
                    {
                        continue; // 避免同名重复
                    }
                    items.push(ModelItem {
                        display_name: format!("[用户] {}", dir_name),
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
    let mut items = Vec::new();

    // 1. 动态扫描系统内置目录: /usr/share/waylan/models/vad/ 或 models/vad/
    if let Some(builtin_dir) = builtin_models_dir() {
        let vad_dir = builtin_dir.join("vad");
        if let Ok(entries) = fs::read_dir(vad_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().map(|e| e == "onnx").unwrap_or(false) {
                    let file_name = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let display_name = if file_name == "silero_vad.onnx" {
                        "[内置] Silero VAD (神经网络模型)".to_string()
                    } else {
                        format!("[内置] {}", file_name)
                    };
                    items.push(ModelItem {
                        display_name,
                        path: Some(p.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }

    // 若系统目录未扫描到任何内置 VAD 模型，提供官方推荐项保底
    if items.is_empty() {
        items.push(ModelItem {
            display_name: "[内置] Silero VAD (神经网络模型)".to_string(),
            path: None,
        });
    }

    // 2. 内置纯数学算法项（免模型纯能量门限）
    items.push(ModelItem {
        display_name: "[算法] 纯能量门限 (零模型纯数学 RMS)".to_string(),
        path: Some("__energy__".to_string()),
    });

    // 3. 动态扫描用户私有目录: ~/.local/share/waylan/models/vad/*.onnx
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
                        continue; // 与官方内置同名，保留内置项
                    }
                    items.push(ModelItem {
                        display_name: format!("[用户] {}", file_name),
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
    let mut items = Vec::new();

    // 1. 动态扫描系统内置目录: /usr/share/waylan/models/punct/ 或 models/punct/
    if let Some(builtin_dir) = builtin_models_dir() {
        let punct_dir = builtin_dir.join("punct");
        if let Ok(entries) = fs::read_dir(punct_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().map(|e| e == "onnx").unwrap_or(false) {
                    let file_name = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let display_name =
                        if file_name.contains("ct-transformer") || file_name.contains("model") {
                            "[内置] CT-Transformer 深度标点 (int8)".to_string()
                        } else {
                            format!("[内置] {}", file_name)
                        };
                    items.push(ModelItem {
                        display_name,
                        path: Some(p.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }

    // 若系统目录未扫描到任何内置标点模型，提供官方推荐项保底
    if items.is_empty() {
        items.push(ModelItem {
            display_name: "[内置] CT-Transformer 深度标点 (int8)".to_string(),
            path: None,
        });
    }

    // 2. 内置规则兜底项（免模型纯规则断句）
    items.push(ModelItem {
        display_name: "[规则] 基础句末规则兜底 (无模型)".to_string(),
        path: Some("__rule_only__".to_string()),
    });

    // 3. 动态扫描用户私有目录: ~/.local/share/waylan/models/punct/*.onnx
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
                    if file_name == "punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8.onnx"
                        || file_name == "model.onnx"
                        || file_name == "model.int8.onnx"
                    {
                        continue; // 与官方内置同名，保留内置项即可
                    }
                    items.push(ModelItem {
                        display_name: format!("[用户] {}", file_name),
                        path: Some(p.to_string_lossy().to_string()),
                    });
                }
            }
        }
    }

    items
}

/// 扫描并列出所有可用的人声智能降噪 (NS) 模型
pub fn list_ns_models() -> Vec<ModelItem> {
    vec![
        // 1. 纯代码轻量内置引擎（纯 Rust，零模型文件依赖）
        ModelItem {
            display_name: "[内置] RNNoise 智能降噪 (轻量日常，0文件开销)".to_string(),
            path: Some("__rnnoise__".to_string()),
        },
        // 2. 高阶深度消音（强力消除机械键盘打字声）
        ModelItem {
            display_name: "[深度] DeepFilterNet3 (强力消除机械键盘)".to_string(),
            path: Some("__deepfilternet__".to_string()),
        },
        // 3. 新一代 SOTA 神经网络深度降噪 (523KB 极速)
        ModelItem {
            display_name: "[SOTA] GTCRN 深度降噪 (523KB 极速)".to_string(),
            path: Some("__gtcrn__".to_string()),
        },
        // 4. 直通透传项（关闭降噪）
        ModelItem {
            display_name: "[直通] 关闭人声降噪 (Bypass)".to_string(),
            path: Some("__bypass__".to_string()),
        },
    ]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_asr_models_dynamic_scan() {
        let models = list_asr_models();
        assert!(!models.is_empty(), "ASR 扫描列表不应为空");
        assert!(
            models[0].display_name.contains("[内置]"),
            "第一项应为内置模型: {}",
            models[0].display_name
        );
    }

    #[test]
    fn test_list_vad_models_dynamic_scan() {
        let models = list_vad_models();
        assert!(models.len() >= 2, "VAD 应至少包含内置模型与纯能量算法");
        assert!(
            models
                .iter()
                .any(|m| m.path.as_deref() == Some("__energy__")),
            "VAD 必须包含纯能量门限算法项"
        );
    }

    #[test]
    fn test_list_punct_models_dynamic_scan() {
        let models = list_punct_models();
        assert!(models.len() >= 2, "标点应至少包含内置模型与规则兜底项");
        assert!(
            models
                .iter()
                .any(|m| m.path.as_deref() == Some("__rule_only__")),
            "标点必须包含规则兜底项"
        );
    }

    #[test]
    fn test_list_ns_models_dynamic_scan() {
        let models = list_ns_models();
        assert!(
            models.len() >= 3,
            "NS 必须包含深度消音、RNNoise轻量与直通模式"
        );
        assert!(
            models
                .iter()
                .any(|m| m.path.as_deref() == Some("__rnnoise__")),
            "NS 必须包含 RNNoise 纯代码轻量项"
        );
        assert!(
            models
                .iter()
                .any(|m| m.path.as_deref() == Some("__bypass__")),
            "NS 必须包含直通透传项"
        );
    }
}
