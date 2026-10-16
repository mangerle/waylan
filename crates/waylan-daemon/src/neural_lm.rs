//! 本地轻量级神经网络语言模型（16M INT8）环境探测、单例加载与热重载缓存模块
//!
//! # 设计原理
//! - **实现初衷**：微澜输入法在拼音整句解码与下一词预测时引入本地神经因果语言模型进行重排序消歧。
//!   模型文件与分词器体积约为 16MB，热重载或高频按键刷新时绝不可重复解析加载文件。
//! - **核心优势**：通过文件修改时间戳（mtime）感知的单例缓存机制，只有在模型文件被外部覆盖或开关变更时
//!   才触发重新载入，其余时刻均为纳秒级只读零拷贝共享；当权重文件缺失或损坏时，自动平滑保底回退至 `NoopReranker`。
//! - **代价与局限**：首次冷启动加载模型权重需占用约数十毫秒，但后续推理单句耗时仅为毫秒级。

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use log::{info, warn};
use waylan_algo::{NoopReranker, SentenceReranker};
use waylan_lm::NeuralReranker;

use crate::config::AppConfig;

/// 神经网络语言模型单例缓存包装体
struct NeuralRerankerCache {
    enabled: bool,
    model_path: Option<PathBuf>,
    model_mtime: Option<SystemTime>,
    tokenizer_path: Option<PathBuf>,
    tokenizer_mtime: Option<SystemTime>,
    reranker: Arc<dyn SentenceReranker>,
}

static CACHED_NEURAL_RERANKER: OnceLock<Mutex<NeuralRerankerCache>> = OnceLock::new();

/// 探测当前系统中生效的神经网络语言模型权重与分词器路径及其修改时间戳
fn probe_active_lm_meta() -> (
    Option<PathBuf>,
    Option<SystemTime>,
    Option<PathBuf>,
    Option<SystemTime>,
) {
    let mut model_pair = (None, None);
    for path in AppConfig::lm_model_search_paths() {
        if path.is_file() {
            let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            model_pair = (Some(path), mtime);
            break;
        }
    }

    let mut tok_pair = (None, None);
    for path in AppConfig::lm_tokenizer_search_paths() {
        if path.is_file() {
            let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            tok_pair = (Some(path), mtime);
            break;
        }
    }

    (model_pair.0, model_pair.1, tok_pair.0, tok_pair.1)
}

/// 从磁盘构建神经网络语言模型重排序器缓存实例
fn build_neural_reranker(enabled: bool) -> NeuralRerankerCache {
    if !enabled {
        info!("[神经语言模型] 配置已显式禁用神经网络语言模型，使用系统保底空操作重排序器。");
        return NeuralRerankerCache {
            enabled: false,
            model_path: None,
            model_mtime: None,
            tokenizer_path: None,
            tokenizer_mtime: None,
            reranker: Arc::new(NoopReranker),
        };
    }

    let (model_path, model_mtime, tokenizer_path, tokenizer_mtime) = probe_active_lm_meta();
    match (&model_path, &tokenizer_path) {
        (Some(mp), Some(tp)) => {
            let load_start = Instant::now();
            match NeuralReranker::load_from_files(mp, tp, None) {
                Ok(loaded) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "[神经语言模型] 成功加载本地轻量 16M 神经网络语言模型 (耗时: {:.2} ms, 权重: {:?}, 分词器: {:?})",
                        elapsed.as_secs_f64() * 1000.0,
                        mp,
                        tp
                    );
                    NeuralRerankerCache {
                        enabled: true,
                        model_path,
                        model_mtime,
                        tokenizer_path,
                        tokenizer_mtime,
                        reranker: Arc::new(loaded),
                    }
                }
                Err(e) => {
                    warn!(
                        "[神经语言模型 警告] 读取神经网络语言模型权重或分词器失败: {}，回退到保底空操作重排序器。",
                        e
                    );
                    NeuralRerankerCache {
                        enabled: true,
                        model_path,
                        model_mtime,
                        tokenizer_path,
                        tokenizer_mtime,
                        reranker: Arc::new(NoopReranker),
                    }
                }
            }
        }
        _ => {
            info!("[神经语言模型] 未检测到本地 16M 神经网络模型权重或分词器，使用系统保底空操作重排序器。");
            NeuralRerankerCache {
                enabled: true,
                model_path,
                model_mtime,
                tokenizer_path,
                tokenizer_mtime,
                reranker: Arc::new(NoopReranker),
            }
        }
    }
}

/// 按照多级搜索策略加载神经网络语言模型重排序器
///
/// 具备时间戳感知与配置变更感知：未变动时零拷贝复用，发生变动时自适应热重载。
pub fn load_neural_reranker(enabled: bool) -> Arc<dyn SentenceReranker> {
    let cache_mutex =
        CACHED_NEURAL_RERANKER.get_or_init(|| Mutex::new(build_neural_reranker(enabled)));

    let (m_path, m_mtime, t_path, t_mtime) = probe_active_lm_meta();
    if let Ok(mut guard) = cache_mutex.lock() {
        if guard.enabled == enabled
            && guard.model_path == m_path
            && guard.model_mtime == m_mtime
            && guard.tokenizer_path == t_path
            && guard.tokenizer_mtime == t_mtime
        {
            return guard.reranker.clone();
        }
        let fresh = build_neural_reranker(enabled);
        let out = fresh.reranker.clone();
        *guard = fresh;
        return out;
    }

    build_neural_reranker(enabled).reranker
}

#[cfg(test)]
mod tests {
    use super::*;
    use waylan_algo::AlgoCandidate;

    #[test]
    fn test_load_neural_reranker_fallback_noop() {
        // 当未提供外部真实模型文件时，必须平滑保底返回 NoopReranker，不 panic 且行为正确
        let reranker = load_neural_reranker(true);
        let mut candidates = vec![
            AlgoCandidate::new("西安", "xi'an", 100),
            AlgoCandidate::new("熄安", "xi'an", 80),
        ];
        reranker.rerank(&mut candidates, None);
        // 保底实现不改变候选顺序与分数
        assert_eq!(candidates[0].text, "西安");
        assert_eq!(candidates[1].text, "熄安");
    }

    #[test]
    fn test_load_neural_reranker_disabled() {
        let reranker = load_neural_reranker(false);
        let mut candidates = vec![AlgoCandidate::new("测试", "ce'shi", 10)];
        reranker.rerank(&mut candidates, None);
        assert_eq!(candidates[0].text, "测试");
    }
}
