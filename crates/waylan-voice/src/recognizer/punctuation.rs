//! 离线标点符号预测还原器（基于 sherpa-onnx CT-Transformer 标点模型）

use std::path::PathBuf;

use log::{info, warn};
use sherpa_onnx::{OfflinePunctuation, OfflinePunctuationConfig};

/// 离线标点符号预测还原器（基于 sherpa-onnx CT-Transformer 标点模型）
pub struct PunctuationRestorer {
    model: Option<OfflinePunctuation>,
}

impl PunctuationRestorer {
    /// 创建标点符号还原器，自动探测本地模型文件
    pub fn new() -> Self {
        Self::with_custom_path(None)
    }

    /// 创建标点符号还原器，优先使用指定的自定义模型路径，未指定时自动探测系统与用户默认模型
    pub fn with_custom_path(custom_path: Option<&str>) -> Self {
        let model = Self::try_create_model(custom_path);
        Self { model }
    }

    /// 尝试定位并加载标点模型（严格根据用户在设置界面的选择加载）
    fn try_create_model(custom_path: Option<&str>) -> Option<OfflinePunctuation> {
        let model_path = match custom_path.filter(|s| !s.trim().is_empty()) {
            Some("__rule_only__") => {
                info!("[标点还原] 用户已在设置中心选择纯规则兜底，禁用神经网络标点模型");
                return None;
            }
            Some(path) => {
                let pb = PathBuf::from(path);
                if pb.is_file() {
                    info!("[标点还原] 正在加载用户指定的标点模型: {:?}", pb);
                    Some(pb)
                } else {
                    warn!(
                        "[标点还原] 用户指定的标点模型文件不存在: {:?}，正在回退至系统内置模型",
                        pb
                    );
                    Self::resolve_builtin_punct_model_path()
                }
            }
            None => {
                // 默认使用官方系统内置模型
                Self::resolve_builtin_punct_model_path()
            }
        }?;
        info!("[标点还原] 检测到本地标点模型文件: {:?}", model_path);
        let mut config = OfflinePunctuationConfig::default();
        config.model.ct_transformer = Some(model_path.to_string_lossy().to_string());
        config.model.num_threads = 1;
        config.model.provider = Some("cpu".to_string());

        match OfflinePunctuation::create(&config) {
            Some(p) => {
                info!("[标点还原] 成功创建 sherpa-onnx 离线标点符号预测模型");
                Some(p)
            }
            None => {
                warn!("[标点还原] 初始化标点模型失败，将回退到基础标点规则");
                None
            }
        }
    }

    /// 定位系统官方内置标点模型权重文件
    fn resolve_builtin_punct_model_path() -> Option<PathBuf> {
        let filename = "punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8.onnx";
        let mut candidates = Vec::new();
        candidates.push(PathBuf::from("/usr/share/waylan/models/punct").join(filename));
        candidates.push(PathBuf::from("models/punct").join(filename));
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(
                PathBuf::from(&home)
                    .join(".local/share/waylan/models/punct")
                    .join(filename),
            );
        }
        // 向前兼容旧版全精度文件名
        candidates.push(PathBuf::from("/usr/share/waylan/models/punct/model.onnx"));
        candidates.push(PathBuf::from("models/punct/model.onnx"));

        candidates.into_iter().find(|p| p.is_file())
    }

    /// 为文本预测并添加标点符号；若无可用模型，执行基础句末补全
    pub fn restore(&self, text: &str) -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }

        if let Some(ref punct) = self.model {
            if let Some(res) = punct.add_punctuation(trimmed) {
                let clean = res.trim();
                if !clean.is_empty() {
                    return clean.to_string();
                }
            }
        }

        // 基础兜底规则：末尾无标点时补充中文句号
        let mut fallback = trimmed.to_string();
        if !fallback.ends_with('。')
            && !fallback.ends_with('，')
            && !fallback.ends_with('！')
            && !fallback.ends_with('？')
            && !fallback.ends_with('；')
        {
            fallback.push('。');
        }
        fallback
    }
}

impl Default for PunctuationRestorer {
    fn default() -> Self {
        Self::new()
    }
}
