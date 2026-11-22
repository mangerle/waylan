//! 本地语音模型扫描发现、用户目录物理隔离与外部导入管理模块
//!
//! # 设计原理
//! - **实现初衷**：为输入法提供本地模型统一的发现扫描、类型分类与无损导入能力。
//! - **核心优势**：严格遵循 Linux XDG 规范，实现系统内置目录与用户导入目录的物理隔离。
//! - **模块化裂变**：按关注点分离为 `scanner`（模型发现扫描）与 `importer`（物理导入与目录管理）。

pub mod downloader;
pub mod importer;
pub mod scanner;

pub use downloader::{
    download_and_extract_model, find_preset_model_by_id, format_download_speed,
    get_preset_model_dest_dir, is_preset_model_installed, AsrModelInstallState, OnlineAsrModelMeta,
    PRESET_ASR_MODELS,
};
pub use importer::{
    builtin_models_dir, ensure_user_models_dirs, import_asr_model_from_dir,
    import_onnx_model_from_file, open_user_models_directory, user_models_dir,
};
pub use scanner::{list_asr_models, list_ns_models, list_vad_models};

/// 可供用户选择的模型项描述
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelItem {
    /// 界面友好展示名称
    pub display_name: String,
    /// 对应的配置文件存储路径；为 None 时代表自动使用系统内置或默认模型
    pub path: Option<String>,
}

/// 用户模型分类子目录枚举，确保编译期类型安全与文件路径防穿越
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSubdirectory {
    /// 语音活动检测模型目录
    Vad,
    /// 智能降噪模型目录
    Ns,
    /// 语音识别模型目录
    Asr,
}

impl ModelSubdirectory {
    /// 获取受控的相对路径目录名称
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vad => "vad",
            Self::Ns => "ns",
            Self::Asr => "asr",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_list_asr_models_dynamic_scan() {
        let models = list_asr_models();
        assert!(!models.is_empty(), "ASR 扫描列表不应为空");
        assert!(
            models[0].display_name.contains("[内置]") || models[0].display_name.contains("[推荐]"),
            "第一项应为内置或推荐模型: {}",
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

    #[test]
    fn test_import_models_pure_io() {
        let temp_dir = tempfile::tempdir().unwrap();
        let src_model_dir = temp_dir.path().join("dummy_asr");
        fs::create_dir_all(&src_model_dir).unwrap();
        fs::write(src_model_dir.join("model.onnx"), b"dummy").unwrap();

        let imported_dir = import_asr_model_from_dir(&src_model_dir);
        assert!(imported_dir.is_some());
        let imported_path = imported_dir.unwrap();
        assert!(imported_path.exists());
        assert!(imported_path.join("model.onnx").exists());

        // 清理测试产物
        let _ = fs::remove_dir_all(&imported_path);

        let src_onnx = temp_dir.path().join("dummy_vad.onnx");
        fs::write(&src_onnx, b"dummy vad onnx").unwrap();
        let imported_vad = import_onnx_model_from_file(ModelSubdirectory::Vad, &src_onnx);
        assert!(imported_vad.is_some());
        let imported_vad_path = imported_vad.unwrap();
        assert!(imported_vad_path.exists());
        let _ = fs::remove_file(&imported_vad_path);
    }
}
