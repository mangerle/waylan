//! 本地语音模型目录动态发现、类型分类与目录扫描
//!
//! # 设计原理
//! - **实现初衷**：为输入法前端与设置中心提供本地可用 ASR、VAD 与降噪模型目录的统一动态发现。
//! - **核心优势**：自动合并系统内置与用户私有目录，按模型类型进行同名去重与推荐权重排序。
//! - **代价与局限**：涉及少量同步目录元数据读取，仅在设置界面打开或配置查询时低频调用。

use std::fs;
use std::path::{Path, PathBuf};

use super::importer::{builtin_models_dir, user_models_dir};
use super::ModelItem;

/// 扫描指定目录下的 `.onnx` 模型文件，返回 `(文件名, 完整路径)` 列表
///
/// # 设计原理
/// - **实现初衷**：VAD、标点等模型目录均遵循同一 `.onnx` 文件约定，收敛扫描逻辑以消除重复的目录遍历样板代码。
/// - **核心优势**：统一处理目录不可读、条目读取失败与非 `.onnx` 文件过滤，调用方仅需关注展示名拼装。
/// - **代价与局限**：目录不存在时静默返回空列表，交由调用方决定兜底策略。
fn scan_onnx_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().is_some_and(|ext| ext == "onnx"))
        .map(|path| {
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            (file_name, path)
        })
        .collect()
}

/// 扫描指定目录下的模型子目录，返回 `(目录名, 完整路径)` 列表
///
/// # 设计原理
/// - **实现初衷**：ASR 模型以「目录」为单位分发，与 `.onnx` 单文件模型形态不同，故独立提供目录级扫描入口。
/// - **核心优势**：与 [`scan_onnx_files`] 保持一致的失败静默与条目过滤语义，避免两套扫描逻辑行为漂移。
/// - **代价与局限**：仅返回一级子目录，不递归下探。
fn scan_subdirectories(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| {
            let dir_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            (dir_name, path)
        })
        .collect()
}

/// 扫描并列出所有可用的 ASR 识别模型（动态扫描系统内置目录与用户导入目录）
pub fn list_asr_models() -> Vec<ModelItem> {
    let mut items = Vec::new();

    // 1. 动态扫描系统内置目录: /usr/share/waylan/models/asr/ 或 models/asr/
    if let Some(builtin_dir) = builtin_models_dir() {
        for (dir_name, path) in scan_subdirectories(&builtin_dir.join("asr")) {
            items.push(ModelItem {
                display_name: format!("[内置] {}", dir_name),
                path: Some(path.to_string_lossy().to_string()),
            });
        }
    }

    // 优先将 SenseVoice-Small、Qwen3-ASR、FireRedASR2 与 Fun-ASR-Nano 推荐模型排在首位
    let score_model = |name: &str| {
        let lower = name.to_lowercase();
        if lower.contains("sensevoice") {
            4
        } else if lower.contains("qwen3") {
            3
        } else if lower.contains("firered") || lower.contains("fire-red") {
            2
        } else if lower.contains("funasr") {
            1
        } else {
            0
        }
    };
    items.sort_by_key(|a| std::cmp::Reverse(score_model(&a.display_name)));

    // 若系统目录未扫描到任何内置 ASR 模型，提供默认推荐项保底
    if items.is_empty() {
        items.push(ModelItem {
            display_name: "[推荐] sensevoice-small".to_string(),
            path: None,
        });
    }

    // 2. 动态扫描用户私有目录: ~/.local/share/waylan/models/asr/*/
    if let Some(user_dir) = user_models_dir() {
        for (dir_name, path) in scan_subdirectories(&user_dir.join("asr")) {
            let path_str = path.to_string_lossy().to_string();
            if items
                .iter()
                .any(|m| m.path.as_deref() == Some(path_str.as_str()))
            {
                continue; // 避免同名重复
            }
            items.push(ModelItem {
                display_name: format!("[用户] {}", dir_name),
                path: Some(path_str),
            });
        }
    }

    // 再次按推荐权重稳定排序
    items.sort_by_key(|a| std::cmp::Reverse(score_model(&a.display_name)));

    items
}

/// 扫描并列出所有可用的 VAD 端点检测模型/算法
pub fn list_vad_models() -> Vec<ModelItem> {
    let mut items = Vec::new();

    // 1. 动态扫描系统内置目录: /usr/share/waylan/models/vad/ 或 models/vad/
    if let Some(builtin_dir) = builtin_models_dir() {
        for (file_name, path) in scan_onnx_files(&builtin_dir.join("vad")) {
            let display_name = if file_name == "silero_vad.onnx" {
                "[内置] Silero VAD (神经网络模型)".to_string()
            } else {
                format!("[内置] {}", file_name)
            };
            items.push(ModelItem {
                display_name,
                path: Some(path.to_string_lossy().to_string()),
            });
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
        for (file_name, path) in scan_onnx_files(&user_dir.join("vad")) {
            if file_name == "silero_vad.onnx" {
                continue; // 与官方内置同名，保留内置项
            }
            items.push(ModelItem {
                display_name: format!("[用户] {}", file_name),
                path: Some(path.to_string_lossy().to_string()),
            });
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
