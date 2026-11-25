//! 词库概况与统计查询服务门面

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use waylan_algo::custom_phrase::AuxiliaryDict;
use waylan_algo::table::CompactMmapLexicon;
use waylan_algo::user_adaptive::UserAdaptiveModel;
use waylan_core::config::AppConfig;

use super::loader::load_auxiliary_dict;

/// 用户词库条目数缓存签名 `(路径, 文件字节数, 最近修改时间, 条目数)`
type UserDictCountCache = (PathBuf, u64, Option<SystemTime>, usize);

static USER_DICT_COUNT_CACHE: OnceLock<Mutex<Option<UserDictCountCache>>> = OnceLock::new();

/// 词库概况与统计查询服务门面
///
/// # 设计原理
/// - **实现初衷**：为偏好设置中心（waylan-settings）与外部诊断工具提供统一的词库状态查询入口，
///   消除上层直接触碰底层 `CompactMmapLexicon` 或 `UserAdaptiveModel` 的穿透性依赖坏味道。
/// - **核心优势**：集中收敛系统词库与自造词模型统计口径，并通过文件签名缓存避免重复 WAL 全量重放。
pub struct LexiconSummaryService;

impl LexiconSummaryService {
    /// 查询系统预编译主词库（base.dict.bin）的状态描述信息
    pub fn query_system_dict_info() -> String {
        for path in AppConfig::base_dict_search_paths() {
            if path.exists()
                && let Ok(lexicon) = CompactMmapLexicon::open(&path)
            {
                return format!(
                    "系统预编译词库: base.dict.bin (约 {:.1} 万组词条已就绪)",
                    lexicon.num_candidates as f64 / 10_000.0
                );
            }
        }
        "系统预编译词库: base.dict.bin (未找到或未就绪)".to_string()
    }

    /// 查询用户自适应个性化历史词模型的记录总数（基于文件签名缓存，未修改时跳过 WAL 全量重放）
    pub fn query_user_adaptive_entry_count() -> usize {
        let user_dict_path = AppConfig::user_dict_path();
        let Ok(meta) = fs::metadata(&user_dict_path) else {
            return 0;
        };
        let len = meta.len();
        let modified = meta.modified().ok();
        let cache = USER_DICT_COUNT_CACHE.get_or_init(|| Mutex::new(None));

        if let Ok(guard) = cache.lock()
            && let Some((ref cached_path, cached_len, cached_mod, cached_count)) = *guard
            && cached_path == &user_dict_path
            && cached_len == len
            && cached_mod == modified
        {
            return cached_count;
        }

        let count = UserAdaptiveModel::load_or_replay_wal(&user_dict_path)
            .map(|m| m.total_entries())
            .unwrap_or(0);
        if let Ok(mut guard) = cache.lock() {
            *guard = Some((user_dict_path, len, modified, count));
        }
        count
    }

    /// 加载并返回辅助混输词库（英文单词与特殊符号）
    pub fn load_auxiliary_dict() -> AuxiliaryDict {
        load_auxiliary_dict()
    }
}
