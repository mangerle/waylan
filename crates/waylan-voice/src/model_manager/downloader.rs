//! 预置离线语音识别模型在线元数据与异步流式下载管理模块
//!
//! # 设计原理
//! - **实现初衷**：为输入法图形设置中心提供预置优质 ASR 模型的元数据索引、本地安装状态检测
//!   与基于国内高速镜像（ModelScope）的后台无卡顿异步下载与自动解压能力。
//! - **核心优势**：用户无需打开终端手动输入命令或查找深层系统路径，在界面中即可一键完成
//!   模型下载、自动解压规整与无缝切换；下载过程严格运行于独立后台线程并实时汇报进度。
//! - **安全防范**：所有下载目标目录严格限定在遵循 XDG 规范的用户模型私有目录下，杜绝路径穿越。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use log::{info, warn};
use waylan_core::OfflineModelType;

use super::importer::{builtin_models_dir, user_models_dir};
use crate::error::VoiceError;

/// 官方精选支持的离线 ASR 模型元数据定义
#[derive(Debug, Clone, Copy)]
pub struct OnlineAsrModelMeta {
    /// 唯一模型标识
    pub id: &'static str,
    /// 界面友好展示名称
    pub name: &'static str,
    /// 特性徽章胶囊（如：极速推荐、语义纠错、方言首选）
    pub badge: &'static str,
    /// 模型体积与推理延迟预估
    pub size_desc: &'static str,
    /// 核心功能与适用场景中文说明
    pub desc: &'static str,
    /// ModelScope / GitHub Releases 对应的发布包前缀
    pub archive_name: &'static str,
    /// 部署到本地时的目标目录名称（相对 asr/ 目录）
    pub dir_name: &'static str,
    /// 对应的本地离线模型架构类型
    pub model_type: OfflineModelType,
}

/// 预置的精选离线语音模型清单（按默认推荐顺序排列）
pub static PRESET_ASR_MODELS: &[OnlineAsrModelMeta] = &[
    OnlineAsrModelMeta {
        id: "sensevoice",
        name: "SenseVoice-Small",
        badge: "极速推荐",
        size_desc: "下载 155MB · 解压约 230MB · 延迟 30-50ms",
        desc: "阿里达摩院 2.3 亿参数轻量非自回归模型，中英粤日韩 5 语极速上屏，原生标点与情感检测，日常高频打字首选",
        archive_name: "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17",
        dir_name: "sensevoice-small",
        model_type: OfflineModelType::SenseVoice,
    },
    OnlineAsrModelMeta {
        id: "qwen3-asr",
        name: "Qwen3-ASR (0.6B)",
        badge: "语义纠错",
        size_desc: "下载 838MB · 解压约 1.2GB · 延迟 200-300ms",
        desc: "阿里通义千问 6 亿参数 LLM 底座，支持 30+ 语言及 22 种中文方言，长句上下文理解与生僻同音字纠错极强",
        archive_name: "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25",
        dir_name: "qwen3-asr",
        model_type: OfflineModelType::Qwen3Asr,
    },
    OnlineAsrModelMeta {
        id: "fireredasr2",
        name: "FireRedASR2",
        badge: "方言首选",
        size_desc: "下载 496MB · 解压约 650MB · 延迟 50-80ms",
        desc: "小红书开源中文登顶模型，CTC 流式蒸馏架构，深度覆盖普通话、英语及 20+ 种方言口音混说与中英夹杂",
        archive_name: "sherpa-onnx-fire-red-asr2-ctc-zh_en-int8-2026-02-25",
        dir_name: "fireredasr2",
        model_type: OfflineModelType::FireRedAsr2,
    },
    OnlineAsrModelMeta {
        id: "funasr-nano",
        name: "FunASR-Nano",
        badge: "多方言大模型",
        size_desc: "下载 803MB · 解压约 1.1GB · 延迟 250-350ms",
        desc: "阿里达摩院 8 亿参数语音理解大模型，全面覆盖 7 大方言区与 26 种地方口音，擅长会议长文本与端到端标点排版",
        archive_name: "sherpa-onnx-funasr-nano-int8-2025-12-30",
        dir_name: "funasr-nano",
        model_type: OfflineModelType::FunAsrNano,
    },
];

/// 模型在当前系统中的物理部署状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrModelInstallState {
    /// 尚未下载
    NotDownloaded = 0,
    /// 正在后台异步下载中
    Downloading = 1,
    /// 本地已完整部署，处于可用待启用状态
    Downloaded = 2,
    /// 当前正在生效使用中
    Active = 3,
}

/// 根据 ID 获取预置模型元数据
pub fn find_preset_model_by_id(id: &str) -> Option<&'static OnlineAsrModelMeta> {
    PRESET_ASR_MODELS.iter().find(|m| m.id == id)
}

/// 解析指定预置模型在当前系统中的本地绝对目标安装目录
pub fn get_preset_model_dest_dir(meta: &OnlineAsrModelMeta) -> Option<PathBuf> {
    user_models_dir().map(|base| base.join("asr").join(meta.dir_name))
}

/// 检测指定预置模型当前是否已完整安装在本地（检查用户私有目录或系统内置目录）
pub fn is_preset_model_installed(meta: &OnlineAsrModelMeta) -> bool {
    let mut candidate_dirs = Vec::new();
    if let Some(user_dir) = user_models_dir() {
        candidate_dirs.push(user_dir.join("asr").join(meta.dir_name));
    }
    if let Some(builtin_dir) = builtin_models_dir() {
        candidate_dirs.push(builtin_dir.join("asr").join(meta.dir_name));
    }

    candidate_dirs.iter().any(|dest| {
        if !dest.is_dir() {
            return false;
        }
        match meta.model_type {
            OfflineModelType::SenseVoice => {
                (dest.join("model.int8.onnx").exists() || dest.join("model.onnx").exists())
                    && dest.join("tokens.txt").exists()
            }
            OfflineModelType::Qwen3Asr => {
                (dest.join("conv_frontend.int8.onnx").exists()
                    || dest.join("conv_frontend.onnx").exists())
                    && (dest.join("encoder.int8.onnx").exists()
                        || dest.join("encoder.onnx").exists())
                    && (dest.join("decoder.int8.onnx").exists()
                        || dest.join("decoder.onnx").exists())
            }
            OfflineModelType::FireRedAsr2 => {
                (dest.join("model.int8.onnx").exists()
                    || dest.join("model.onnx").exists()
                    || dest.join("encoder.int8.onnx").exists())
                    && dest.join("tokens.txt").exists()
            }
            OfflineModelType::FunAsrNano => {
                (dest.join("encoder_adaptor.int8.onnx").exists()
                    || dest.join("encoder_adaptor.onnx").exists())
                    && (dest.join("llm.int8.onnx").exists() || dest.join("llm.onnx").exists())
                    && (dest.join("embedding.int8.onnx").exists()
                        || dest.join("embedding.onnx").exists())
            }
        }
    })
}

/// 格式化传输速率字符串（例如：12.5 MB/s 或 850 KB/s）
pub fn format_download_speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bytes_per_sec / (1024.0 * 1024.0))
    } else if bytes_per_sec >= 1024.0 {
        format!("{:.0} KB/s", bytes_per_sec / 1024.0)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

/// 执行从网络下载模型压缩包并自动解压部署的完整流程
///
/// # 设计原理
/// - 优先从阿里云 ModelScope 国内高速源下载 `.tar.bz2` 文件；若失败自动回退至 GitHub Releases 官方源；
/// - 流式写入临时文件，并通过 `progress_fn` 周期性回调下载进度百分比（0.0 ~ 1.0）与下载速度描述；
/// - 下载完毕后调用原生 `tar` 命令解压，规整并原子移动至用户标准模型目录。
pub fn download_and_extract_model<F>(
    model_id: &str,
    mut progress_fn: F,
) -> Result<PathBuf, VoiceError>
where
    F: FnMut(f32, &str),
{
    let meta =
        find_preset_model_by_id(model_id).ok_or_else(|| VoiceError::ModelManagementFailed {
            message: format!("未找到 ID 为 [{}] 的预置离线模型", model_id),
        })?;

    let dest_dir =
        get_preset_model_dest_dir(meta).ok_or_else(|| VoiceError::ModelManagementFailed {
            message: "无法获取用户离线模型目标存储目录".to_string(),
        })?;

    let temp_dir = tempfile::tempdir().map_err(|e| VoiceError::ModelManagementFailed {
        message: format!("创建临时下载工作目录失败: {}", e),
    })?;
    let temp_archive_path = temp_dir
        .path()
        .join(format!("{}.tar.bz2", meta.archive_name));

    // 优先采用 ModelScope 国内高速文件下载源（针对社区已托管模型）
    let modelscope_url = format!(
        "https://modelscope.cn/models/csukuangfj/asr-models/resolve/master/{}.tar.bz2",
        meta.archive_name
    );
    let github_url = format!(
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/{}.tar.bz2",
        meta.archive_name
    );

    info!(
        "[模型下载] 正在尝试从 ModelScope 国内源下载: {}",
        modelscope_url
    );
    let download_res = download_file_with_progress(
        &modelscope_url,
        &temp_archive_path,
        &mut progress_fn,
        "ModelScope",
    );

    if download_res.is_err() {
        warn!(
            "[模型下载] ModelScope 国内源下载未成功，正在尝试 GitHub Releases 兜底: {}",
            github_url
        );
        download_file_with_progress(&github_url, &temp_archive_path, &mut progress_fn, "GitHub")?;
    }

    progress_fn(0.99, "正在解压并规整模型权重...");
    info!("[模型下载] 压缩包下载成功，正在解压至临时目录...");

    // 先列举归档条目校验路径安全，严防包含 `..` 或绝对路径的 Tar Slip 越权穿越
    let archive_str =
        temp_archive_path
            .to_str()
            .ok_or_else(|| VoiceError::ModelManagementFailed {
                message: "模型归档路径包含非法 UTF-8 字符".to_string(),
            })?;
    let dest_str = temp_dir
        .path()
        .to_str()
        .ok_or_else(|| VoiceError::ModelManagementFailed {
            message: "临时解压路径包含非法 UTF-8 字符".to_string(),
        })?;
    verify_tar_archive_safety(archive_str)?;

    // 调用系统 tar 解压（附带 180 秒超时保护，超时强杀并收割子进程）
    run_tar_extract_with_timeout(archive_str, dest_str, Duration::from_secs(180))?;

    // 在临时目录中寻找解压出来的子目录
    let extracted_sub_dir = fs::read_dir(temp_dir.path())
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("遍历解压目录失败: {}", e),
        })?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_dir() && path != &dest_dir)
        .ok_or_else(|| VoiceError::ModelManagementFailed {
            message: "解压后的目录结构不符合预期，未找到内容目录".to_string(),
        })?;

    // 创建最终的目标目录
    if let Some(parent) = dest_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("创建目标模型父级目录失败: {}", e),
        })?;
    }
    if !dest_dir.exists() {
        fs::create_dir_all(&dest_dir).map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("创建目标模型目录失败: {}", e),
        })?;
    }

    // 将解压出的全部文件复制到目标目录
    copy_dir_all(&extracted_sub_dir, &dest_dir).map_err(|e| VoiceError::ModelManagementFailed {
        message: format!("将模型文件同步至目标目录失败: {}", e),
    })?;

    progress_fn(1.0, "模型部署完成");
    info!(
        "[模型下载] 模型 [{}] 成功部署至: {}",
        meta.name,
        dest_dir.display()
    );
    Ok(dest_dir)
}

/// 流式分块下载文件并节流计算进度与速率
fn download_file_with_progress<F>(
    url: &str,
    target_path: &Path,
    progress_fn: &mut F,
    source_label: &str,
) -> Result<(), VoiceError>
where
    F: FnMut(f32, &str),
{
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build();
    let agent: ureq::Agent = config.into();

    let resp = agent
        .get(url)
        .call()
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("连接远程下载源 [{}] 失败: {}", source_label, e),
        })?;

    let total_bytes: Option<u64> = resp.body().content_length();

    let mut reader = resp.into_body().into_reader();
    let mut file = File::create(target_path).map_err(|e| VoiceError::ModelManagementFailed {
        message: format!("创建本地目标文件失败: {}", e),
    })?;

    let mut buffer = [0u8; 64 * 1024]; // 64KB 缓冲区
    let mut downloaded_bytes: u64 = 0;
    let start_time = Instant::now();
    let mut last_progress_time = Instant::now();
    let mut bytes_since_last_tick: u64 = 0;
    let mut speed_desc = format!("正在从 {} 下载...", source_label);

    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| VoiceError::ModelManagementFailed {
                message: format!("数据传输过程中读取网络流中断: {}", e),
            })?;

        if n == 0 {
            break;
        }

        file.write_all(&buffer[..n])
            .map_err(|e| VoiceError::ModelManagementFailed {
                message: format!("写入磁盘失败: {}", e),
            })?;

        downloaded_bytes += n as u64;
        bytes_since_last_tick += n as u64;

        // 每隔 150 毫秒节流更新一次进度与下载速率，避免高频刷屏
        let now = Instant::now();
        let elapsed = now.duration_since(last_progress_time);
        if elapsed >= Duration::from_millis(150) {
            let speed = (bytes_since_last_tick as f64) / elapsed.as_secs_f64();
            bytes_since_last_tick = 0;
            last_progress_time = now;

            let formatted_speed = format_download_speed(speed);
            let fraction = match total_bytes {
                Some(total) if total > 0 => {
                    let f = (downloaded_bytes as f32) / (total as f32);
                    let percent = (f * 100.0).clamp(0.0, 99.0);
                    speed_desc = format!("{:.1}% · {}", percent, formatted_speed);
                    f.clamp(0.0, 0.98)
                }
                _ => {
                    speed_desc = format!(
                        "已下载 {:.1} MB · {}",
                        (downloaded_bytes as f64) / (1024.0 * 1024.0),
                        formatted_speed
                    );
                    0.5
                }
            };

            progress_fn(fraction, &speed_desc);
        }
    }

    file.flush()
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("刷新磁盘缓存失败: {}", e),
        })?;

    let total_secs = start_time.elapsed().as_secs_f64().max(0.001);
    let avg_speed = (downloaded_bytes as f64) / total_secs;
    info!(
        "[模型下载] 下载完成，共计 {:.2} MB，平均速率: {}",
        (downloaded_bytes as f64) / (1024.0 * 1024.0),
        format_download_speed(avg_speed)
    );

    Ok(())
}

/// 校验 tar.bz2 归档内所有条目路径安全，严防绝对路径或 `..` 路径穿越（Tar Slip）
fn verify_tar_archive_safety(archive_path: &str) -> Result<(), VoiceError> {
    let output = Command::new("tar")
        .args(["-tjf", archive_path])
        .output()
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("校验模型归档目录失败: {}", e),
        })?;
    if !output.status.success() {
        return Err(VoiceError::ModelManagementFailed {
            message: "校验模型归档失败，可能文件损坏或格式不受支持".to_string(),
        });
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    for raw_entry in listing.lines() {
        let entry = raw_entry.trim();
        if entry.is_empty() {
            continue;
        }
        let path = Path::new(entry);
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(VoiceError::ModelManagementFailed {
                message: format!("模型归档包含不安全的越权路径条目: {}", entry),
            });
        }
    }
    Ok(())
}

/// 在指定超时限制内执行 `tar -xjf` 解压，超时强制终止并收割子进程
fn run_tar_extract_with_timeout(
    archive_path: &str,
    dest_dir: &str,
    timeout: Duration,
) -> Result<(), VoiceError> {
    let mut child = Command::new("tar")
        .args(["-xjf", archive_path, "-C", dest_dir])
        .spawn()
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("启动 tar 解压模型失败: {}", e),
        })?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(VoiceError::ModelManagementFailed {
                        message: "解压模型归档文件失败，可能文件损坏或缺少解压工具".to_string(),
                    });
                }
                return Ok(());
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(VoiceError::ModelManagementFailed {
                        message: "解压模型归档超时，已安全中止并清理子进程".to_string(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(VoiceError::ModelManagementFailed {
                    message: format!("等待 tar 解压进程失败: {}", e),
                });
            }
        }
    }
}

/// 递归将源目录下的所有文件和子目录复制到目标目录
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target_path)?;
        } else {
            fs::copy(entry.path(), target_path)?;
        }
    }
    Ok(())
}
