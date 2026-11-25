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
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use log::{info, warn};
use waylan_core::OfflineModelType;

use super::archive::extract_tar_bz2_safely;
use super::importer::{builtin_models_dir, copy_dir_all, user_models_dir};
use crate::error::VoiceError;
use crate::recognizer::is_offline_model_dir_complete;

/// 模型压缩包最大允许下载体积（2 GiB），防止异常/恶意响应耗尽本地磁盘
const MAX_MODEL_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 全局复用模型下载 HTTP 客户端（共享连接池与 TLS 会话）
fn download_http_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_per_read(Some(Duration::from_secs(60)))
            .max_idle_connections(4)
            .max_idle_connections_per_host(2)
            .build();
        config.into()
    })
}

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

    candidate_dirs
        .iter()
        .any(|dest| is_offline_model_dir_complete(dest, meta.model_type))
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
    let parent_dir = dest_dir
        .parent()
        .ok_or_else(|| VoiceError::ModelManagementFailed {
            message: "目标模型目录缺少有效的父级目录".to_string(),
        })?;
    fs::create_dir_all(parent_dir).map_err(|e| VoiceError::ModelManagementFailed {
        message: format!("创建目标模型父级目录失败: {}", e),
    })?;

    // 在同级父目录下建立临时工作区，使后续原子 rename 零拷贝落盘
    let temp_dir = tempfile::Builder::new()
        .prefix(".download-")
        .tempdir_in(parent_dir)
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("创建临时下载工作目录失败: {}", e),
        })?;
    let temp_archive_path = temp_dir
        .path()
        .join(format!("{}.tar.bz2", meta.archive_name));

    download_from_mirrors(meta, &temp_archive_path, &mut progress_fn)?;

    progress_fn(0.99, "正在校验并解压模型权重...");
    info!("[模型下载] 压缩包下载成功，正在安全流式解压至临时目录...");
    extract_tar_bz2_safely(&temp_archive_path, temp_dir.path())?;

    let extracted_sub_dir = find_extracted_model_dir(temp_dir.path(), &dest_dir)?;
    deploy_extracted_model_dir(&extracted_sub_dir, &dest_dir)?;

    progress_fn(1.0, "模型部署完成");
    info!(
        "[模型下载] 模型 [{}] 成功部署至: {}",
        meta.name,
        dest_dir.display()
    );
    Ok(dest_dir)
}

/// 依次尝试从国内镜像源与官方源下载预置模型归档
fn download_from_mirrors<F>(
    meta: &OnlineAsrModelMeta,
    target_path: &Path,
    progress_fn: &mut F,
) -> Result<(), VoiceError>
where
    F: FnMut(f32, &str),
{
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
    if download_file_with_progress(&modelscope_url, target_path, progress_fn, "ModelScope").is_ok()
    {
        return Ok(());
    }

    warn!(
        "[模型下载] ModelScope 国内源下载未成功，正在尝试 GitHub Releases 兜底: {}",
        github_url
    );
    download_file_with_progress(&github_url, target_path, progress_fn, "GitHub")
}

/// 在临时解压工作区中定位唯一模型内容子目录
fn find_extracted_model_dir(work_dir: &Path, dest_dir: &Path) -> Result<PathBuf, VoiceError> {
    fs::read_dir(work_dir)
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("遍历解压目录失败: {}", e),
        })?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_dir() && path != dest_dir)
        .ok_or_else(|| VoiceError::ModelManagementFailed {
            message: "解压后的目录结构不符合预期，未找到内容目录".to_string(),
        })
}

/// 将临时目录中解压完成的模型目录原子部署到目标路径（同文件系统优先零拷贝 rename，跨设备时回退拷贝）
fn deploy_extracted_model_dir(src_dir: &Path, dest_dir: &Path) -> Result<(), VoiceError> {
    let mut backup_dir: Option<PathBuf> = None;
    if dest_dir.exists() {
        let bak = dest_dir.with_extension("bak-deploy");
        if bak.exists() {
            let _ = fs::remove_dir_all(&bak);
        }
        fs::rename(dest_dir, &bak).map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("轮换旧模型目录失败: {}", e),
        })?;
        backup_dir = Some(bak);
    }

    let deploy_res = match fs::rename(src_dir, dest_dir) {
        Ok(()) => Ok(()),
        Err(_) => copy_dir_all(src_dir, dest_dir),
    };

    if let Err(e) = deploy_res {
        if let Some(bak) = backup_dir {
            let _ = fs::remove_dir_all(dest_dir);
            let _ = fs::rename(bak, dest_dir);
        }
        return Err(VoiceError::ModelManagementFailed {
            message: format!("部署模型目录至目标路径失败: {}", e),
        });
    }

    if let Some(bak) = backup_dir {
        let _ = fs::remove_dir_all(bak);
    }
    Ok(())
}

/// 进度节流上报状态追踪器
struct DownloadProgressTracker<'a, F> {
    total_bytes: Option<u64>,
    source_label: &'a str,
    last_tick: Instant,
    bytes_since_tick: u64,
    progress_fn: &'a mut F,
}

impl<F: FnMut(f32, &str)> DownloadProgressTracker<'_, F> {
    fn maybe_report(&mut self, downloaded_bytes: u64, chunk_len: u64) {
        self.bytes_since_tick = self.bytes_since_tick.saturating_add(chunk_len);
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick);
        if elapsed < Duration::from_millis(150) {
            return;
        }
        let speed = (self.bytes_since_tick as f64) / elapsed.as_secs_f64();
        self.bytes_since_tick = 0;
        self.last_tick = now;

        let formatted_speed = format_download_speed(speed);
        let (fraction, desc) = match self.total_bytes {
            Some(total) if total > 0 => {
                let f = (downloaded_bytes as f32) / (total as f32);
                let percent = (f * 100.0).clamp(0.0, 99.0);
                (
                    f.clamp(0.0, 0.98),
                    format!("{:.1}% · {}", percent, formatted_speed),
                )
            }
            _ => (
                0.5,
                format!(
                    "已下载 {:.1} MB · {}",
                    (downloaded_bytes as f64) / (1024.0 * 1024.0),
                    formatted_speed
                ),
            ),
        };
        (self.progress_fn)(fraction, &desc);
    }
}

/// 发起下载 HTTP 请求并校验状态码与声明体积上限
fn open_download_stream(
    url: &str,
    source_label: &str,
) -> Result<(Option<u64>, impl Read), VoiceError> {
    let resp =
        download_http_agent()
            .get(url)
            .call()
            .map_err(|e| VoiceError::ModelManagementFailed {
                message: format!("连接远程下载源 [{}] 失败: {}", source_label, e),
            })?;

    if !resp.status().is_success() {
        return Err(VoiceError::ModelManagementFailed {
            message: format!(
                "远程下载源 [{}] 返回异常状态码: {}",
                source_label,
                resp.status()
            ),
        });
    }

    let total_bytes = resp.body().content_length();
    if let Some(total) = total_bytes
        && total > MAX_MODEL_ARCHIVE_BYTES
    {
        return Err(VoiceError::ModelManagementFailed {
            message: format!(
                "远程模型归档体积 ({} 字节) 超出安全上限 ({} 字节)",
                total, MAX_MODEL_ARCHIVE_BYTES
            ),
        });
    }

    Ok((total_bytes, resp.into_body().into_reader()))
}

/// 将网络响应流分块写入本地文件并触发体积上限校验与节流进度汇报
fn copy_download_stream<R: Read, F: FnMut(f32, &str)>(
    reader: &mut R,
    file: &mut File,
    tracker: &mut DownloadProgressTracker<'_, F>,
) -> Result<u64, VoiceError> {
    let mut buffer = [0u8; 64 * 1024];
    let mut downloaded_bytes: u64 = 0;

    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| VoiceError::ModelManagementFailed {
                message: format!("数据传输过程中读取网络流中断: {}", e),
            })?;
        if n == 0 {
            break;
        }
        downloaded_bytes = downloaded_bytes.saturating_add(n as u64);
        if downloaded_bytes > MAX_MODEL_ARCHIVE_BYTES {
            return Err(VoiceError::ModelManagementFailed {
                message: format!(
                    "模型下载体积超过安全上限 ({} 字节)，已中止传输",
                    MAX_MODEL_ARCHIVE_BYTES
                ),
            });
        }
        file.write_all(&buffer[..n])
            .map_err(|e| VoiceError::ModelManagementFailed {
                message: format!("写入磁盘失败: {}", e),
            })?;
        tracker.maybe_report(downloaded_bytes, n as u64);
    }

    Ok(downloaded_bytes)
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
    let (total_bytes, mut reader) = open_download_stream(url, source_label)?;
    let mut file = File::create(target_path).map_err(|e| VoiceError::ModelManagementFailed {
        message: format!("创建本地目标文件失败: {}", e),
    })?;

    let start_time = Instant::now();
    let mut tracker = DownloadProgressTracker {
        total_bytes,
        source_label,
        last_tick: Instant::now(),
        bytes_since_tick: 0,
        progress_fn,
    };
    let downloaded_bytes = copy_download_stream(&mut reader, &mut file, &mut tracker)?;

    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|e| VoiceError::ModelManagementFailed {
            message: format!("刷新磁盘缓存失败: {}", e),
        })?;

    let avg_speed = (downloaded_bytes as f64) / start_time.elapsed().as_secs_f64().max(0.001);
    info!(
        "[模型下载] 下载完成（源: {}），共计 {:.2} MB，平均速率: {}",
        tracker.source_label,
        (downloaded_bytes as f64) / (1024.0 * 1024.0),
        format_download_speed(avg_speed)
    );
    Ok(())
}
