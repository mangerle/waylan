//! 输入法多级词库加载、符号链接与时间戳感知缓存

use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use log::{info, warn};
use waylan_algo::table::LexiconTable;
#[cfg(test)]
use waylan_algo::OpenCcDict;
use waylan_algo::{AuxiliaryDict, EnglishSuggestionManager, SymbolSuggestionManager};
use waylan_core::AppConfig;

/// 自动建立用户数据目录 (~/.local/share/waylan/) 与外部全量词库的符号链接
pub fn ensure_user_dictionaries_linked() {
    let Some(home) = env::var("HOME").ok().filter(|s| !s.is_empty()) else {
        return;
    };
    let user_data_dir = PathBuf::from(home).join(".local/share/waylan");
    if fs::create_dir_all(&user_data_dir).is_err() {
        return;
    }

    let pairs = [
        ("base.dict.bin", AppConfig::base_dict_search_paths()),
        ("aux.dict.bin", AppConfig::aux_dict_search_paths()),
        ("opencc.dict.bin", AppConfig::opencc_dict_search_paths()),
    ];

    for (file_name, candidates) in pairs {
        let user_file = user_data_dir.join(file_name);
        if !user_file.exists() {
            for cand in candidates {
                if cand.is_file() && cand != user_file {
                    #[cfg(unix)]
                    let link_res = symlink(&cand, &user_file);
                    #[cfg(not(unix))]
                    let link_res = fs::copy(&cand, &user_file).map(|_| ());
                    if link_res.is_ok() {
                        info!(
                            "[字库引擎] 已自动关联词库至用户数据目录: {:?} -> {:?}",
                            user_file, cand
                        );
                        break;
                    }
                }
            }
        }
    }
}

/// 按照多级搜索策略加载字词库
pub fn load_lexicon_table() -> LexiconTable {
    ensure_user_dictionaries_linked();
    info!("[字库引擎] 正在定位并加载字词库索引...");
    let search_paths = AppConfig::base_dict_search_paths();

    for path in &search_paths {
        if path.exists() {
            let load_start = Instant::now();
            match LexiconTable::load_binary(path) {
                Ok(loaded) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "  - 成功加载全量汉语词库: {:?} (耗时: {:.2} ms, 拼音键数: {} 组)",
                        path,
                        elapsed.as_secs_f64() * 1000.0,
                        loaded.len()
                    );
                    return loaded;
                }
                Err(e) => {
                    warn!("  - 读取词库文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    warn!("  - 未检测到有效的基础外部词库文件，回退到内置基础测试词库。");
    LexiconTable::with_sample_data()
}

/// 按照多级搜索策略加载辅助与混输词库（英文与符号表情，供测试使用）
#[cfg(test)]
pub fn load_auxiliary_dict() -> AuxiliaryDict {
    info!("[辅助词库] 正在定位并加载混输与符号词库索引...");
    let search_paths = AppConfig::aux_dict_search_paths();

    for path in &search_paths {
        if path.exists() {
            let load_start = Instant::now();
            match AuxiliaryDict::load_from_file(path) {
                Ok(dict) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "  - 成功加载辅助混输词库: {:?} (耗时: {:.2} ms, 英文词数: {}, 符号表情: {} 组)",
                        path,
                        elapsed.as_secs_f64() * 1000.0,
                        dict.english_words.len(),
                        dict.symbols.len()
                    );
                    return dict;
                }
                Err(e) => {
                    warn!("  - 读取辅助词库文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    info!("  - 未检测到外部辅助词库文件，使用系统内置保底英文与符号表情库。");
    AuxiliaryDict::with_default_data()
}

struct AuxManagerCache {
    source_path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    managers: (Arc<EnglishSuggestionManager>, Arc<SymbolSuggestionManager>),
}

static CACHED_AUX_MANAGERS: OnceLock<Mutex<AuxManagerCache>> = OnceLock::new();

/// 探测当前应当生效的辅助词库文件路径及其最新修改时间
fn probe_active_aux_dict_meta() -> (Option<PathBuf>, Option<SystemTime>) {
    for path in AppConfig::aux_dict_search_paths() {
        if path.exists() {
            let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            return (Some(path), mtime);
        }
    }
    (None, None)
}

/// 从磁盘构建辅助与混输词库管理器缓存实例
fn build_auxiliary_managers() -> AuxManagerCache {
    info!("[辅助词库] 正在定位并初始化辅助与混输词典索引...");
    let search_paths = AppConfig::aux_dict_search_paths();

    for path in search_paths {
        if path.exists() {
            let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());
            let load_start = Instant::now();
            match AuxiliaryDict::load_from_file(&path) {
                Ok(dict) => {
                    let elapsed = load_start.elapsed();
                    info!(
                        "  - 成功加载辅助混输词库: {:?} (耗时: {:.2} ms, 英文词数: {}, 符号表情: {} 组)",
                        path,
                        elapsed.as_secs_f64() * 1000.0,
                        dict.english_words.len(),
                        dict.symbols.len()
                    );
                    let en = Arc::new(EnglishSuggestionManager::from_words(dict.english_words));
                    let sym = Arc::new(SymbolSuggestionManager::from_symbols(dict.symbols));
                    return AuxManagerCache {
                        source_path: Some(path),
                        mtime,
                        managers: (en, sym),
                    };
                }
                Err(e) => {
                    warn!("  - 读取辅助词库文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    info!("  - 未检测到外部辅助词库文件，使用系统内置单例保底英文与符号表情库。");
    AuxManagerCache {
        source_path: None,
        mtime: None,
        managers: (
            EnglishSuggestionManager::default_global(),
            SymbolSuggestionManager::default_global(),
        ),
    }
}

/// 按照多级搜索策略加载辅助与混输词库管理器（带文件时间戳感知的单例缓存，未变动时零拷贝复用，变动时自动热更新）
pub fn load_auxiliary_managers() -> (Arc<EnglishSuggestionManager>, Arc<SymbolSuggestionManager>) {
    let cache_mutex = CACHED_AUX_MANAGERS.get_or_init(|| Mutex::new(build_auxiliary_managers()));

    let (active_path, active_mtime) = probe_active_aux_dict_meta();
    if let Ok(mut guard) = cache_mutex.lock() {
        if guard.source_path == active_path && guard.mtime == active_mtime {
            return guard.managers.clone();
        }
        let fresh = build_auxiliary_managers();
        let out = fresh.managers.clone();
        *guard = fresh;
        return out;
    }

    build_auxiliary_managers().managers
}

/// 按照多级搜索策略加载简繁转换两级词库（复用 OpenCcDict 全局单例缓存，供测试使用）
#[cfg(test)]
pub fn load_opencc_dict() -> Arc<OpenCcDict> {
    OpenCcDict::global()
}
