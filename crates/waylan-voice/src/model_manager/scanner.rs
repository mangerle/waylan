//! 本地语音模型目录动态发现、类型分类与目录扫描
//!
//! # 设计原理
//! - **实现初衷**：为输入法前端与设置中心提供本地可用 ASR、VAD、标点与降噪模型目录的统一动态发现。
//! - **核心优势**：自动合并系统内置与用户私有目录，按模型类型进行同名去重与推荐权重排序。
//! - **代价与局限**：涉及少量同步目录元数据读取，仅在设置界面打开或配置查询时低频调用。

use std::fs;

use super::importer::{builtin_models_dir, user_models_dir};
use super::ModelItem;

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
