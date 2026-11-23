//! 微澜全局资源路径定位与图标文件解析工具
//!
//! # 设计原理
//! - **实现初衷**：为托盘服务、桌面集成以及各协议层提供统一的静态矢量图标资源目录探测与图标规范路径解析能力。
//! - **核心优势**：兼顾开发环境目录结构与 Linux 标准打包安装路径（`/usr/share/waylan/assets`），优先提供规范绝对路径以杜绝宿主桌面环境无法加载图标。
//! - **代价与局限**：首次探测涉及有限次数的文件系统路径存在性校验。

use std::env;
use std::path::{Path, PathBuf};

/// 自动探测微澜默认矢量图标资源目录并验证关键图标文件是否存在
pub fn detect_default_assets_path() -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::with_capacity(8);

    candidates.push(PathBuf::from("assets"));
    candidates.push(PathBuf::from("../assets"));
    candidates.push(PathBuf::from("../../assets"));

    if let Ok(exe) = env::current_exe()
        && let Some(parent) = exe.parent()
    {
        candidates.push(parent.join("assets"));
        candidates.push(parent.join("../assets"));
        candidates.push(parent.join("../../assets"));
        candidates.push(parent.join("../../../assets"));
        candidates.push(parent.join("../share/waylan/assets"));
    }

    candidates.push(PathBuf::from("/usr/share/waylan/assets"));
    candidates.push(PathBuf::from("/usr/local/share/waylan/assets"));

    if let Ok(home) = env::var("HOME") {
        candidates.push(PathBuf::from(home).join(".local/share/waylan/assets"));
    }

    for dir in candidates {
        if dir.is_dir() && dir.join("tray-en-caps.svg").is_file() {
            if let Ok(canonical) = dir.canonicalize() {
                return Some(canonical.to_string_lossy().into_owned());
            }
            return Some(dir.to_string_lossy().into_owned());
        }
    }

    None
}

/// 解析图标目标：若在指定目录存在该名称对应文件（或附带 .svg/.png），则优先返回规范绝对路径以保证系统各托盘直接加载；否则回退为图标名
pub fn resolve_icon_path_or_name(theme_dir: &str, base_name: &str) -> String {
    if !theme_dir.is_empty() {
        let dir = Path::new(theme_dir);
        let direct = dir.join(base_name);
        if direct.is_file() {
            return direct
                .canonicalize()
                .unwrap_or(direct)
                .to_string_lossy()
                .into_owned();
        }
        let svg = dir.join(format!("{}.svg", base_name));
        if svg.is_file() {
            return svg
                .canonicalize()
                .unwrap_or(svg)
                .to_string_lossy()
                .into_owned();
        }
        let png = dir.join(format!("{}.png", base_name));
        if png.is_file() {
            return png
                .canonicalize()
                .unwrap_or(png)
                .to_string_lossy()
                .into_owned();
        }
    }
    base_name.to_string()
}
