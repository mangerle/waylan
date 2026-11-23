//! 配置文件、模型与词库资产的多级路径探测与环境感知（全工程单一真相源）

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// 过滤空白字符串并返回修剪后的非空字符串切片
#[inline]
pub fn filter_non_empty_str(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// 读取非空环境变量（自动过滤空白字符串与未设置变量）
pub fn env_var_non_empty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .and_then(|v| filter_non_empty_str(&v).map(ToString::to_string))
}

/// 辅助根据执行文件所在目录向多级父路径推入候选词库位置（适应开发、测试与各类宿主子进程环境）
fn push_exe_relative_candidates(paths: &mut Vec<PathBuf>, rel_subpath: &str) {
    if let Ok(exe) = env::current_exe()
        && let Some(parent) = exe.parent()
    {
        paths.push(parent.join(rel_subpath));
        paths.push(parent.join("..").join(rel_subpath));
        paths.push(parent.join("../..").join(rel_subpath));
        paths.push(parent.join("../../..").join(rel_subpath));
        paths.push(parent.join("../share/waylan").join(rel_subpath));
    }
}

/// 获取无 HOME 环境下的受控运行时回退目录
/// 优先使用 `XDG_RUNTIME_DIR/waylan`，其次回退到带有当前用户 UID 标识且权限为 0700 的 `/tmp/waylan-<uid>`
pub fn fallback_runtime_dir() -> PathBuf {
    if let Some(runtime_dir) = env_var_non_empty("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(runtime_dir).join("waylan");
        let _ = fs::create_dir_all(&dir);
        return dir;
    }

    #[cfg(unix)]
    let uid = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(1000)
    };
    #[cfg(not(unix))]
    let uid = 1000;

    let dir = env::temp_dir().join(format!("waylan-{}", uid));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        let _ = builder.create(&dir);
    }
    #[cfg(not(unix))]
    {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

/// 获取符合 XDG 规范的用户配置目录（`~/.config/waylan`）
pub fn user_config_dir() -> Option<PathBuf> {
    if let Some(config_home) = env_var_non_empty("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(config_home).join("waylan"));
    }
    if let Some(home) = env_var_non_empty("HOME") {
        return Some(PathBuf::from(home).join(".config").join("waylan"));
    }
    None
}

/// 获取符合 XDG 规范的用户数据目录（无环境变量时返回 `None`）
pub fn user_data_dir_opt() -> Option<PathBuf> {
    if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
        return Some(PathBuf::from(data_home).join("waylan"));
    }
    if let Some(home) = env_var_non_empty("HOME") {
        return Some(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("waylan"),
        );
    }
    None
}

/// 获取符合 XDG 规范的用户数据目录（无 HOME 时回退到受控运行时目录）
pub fn user_data_dir() -> PathBuf {
    user_data_dir_opt().unwrap_or_else(fallback_runtime_dir)
}

/// 获取符合 XDG 规范的用户状态目录（`~/.local/state/waylan`）
pub fn user_state_dir() -> PathBuf {
    if let Some(state_home) = env_var_non_empty("XDG_STATE_HOME") {
        return PathBuf::from(state_home).join("waylan");
    }
    if let Some(home) = env_var_non_empty("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("waylan");
    }
    fallback_runtime_dir()
}

/// 获取符合 XDG 规范的用户个人模型存储根目录（`~/.local/share/waylan/models`）
pub fn user_models_dir() -> Option<PathBuf> {
    user_data_dir_opt().map(|d| d.join("models"))
}

/// 统一生成数据资产的多级查找路径（环境变量覆盖 -> XDG 用户数据目录 -> 系统目录 -> 相对路径）
fn data_file_search_paths(env_override_key: Option<&str>, file_name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(16);
    if let Some(key) = env_override_key
        && let Some(custom) = env_var_non_empty(key)
    {
        paths.push(PathBuf::from(custom));
    }
    if let Some(user_dir) = user_data_dir_opt() {
        paths.push(user_dir.join(file_name));
    }
    let data_rel = format!("data/{}", file_name);
    paths.push(PathBuf::from("/usr/share/waylan").join(&data_rel));
    paths.push(PathBuf::from("/usr/share/waylan").join(file_name));
    paths.push(PathBuf::from("/usr/local/share/waylan").join(&data_rel));
    paths.push(PathBuf::from("/usr/local/share/waylan").join(file_name));
    push_exe_relative_candidates(&mut paths, &data_rel);
    push_exe_relative_candidates(&mut paths, file_name);
    paths.push(PathBuf::from(&data_rel));
    paths.push(PathBuf::from(file_name));
    paths
}

/// 统一解析语音/AI 模型路径（自定义路径 -> XDG 用户模型目录 -> 系统模型目录 -> 源码相对目录）
pub fn resolve_model_path(custom_path: Option<&Path>, subpaths: &[&str]) -> Option<PathBuf> {
    if let Some(p) = custom_path
        && p.exists()
    {
        return Some(p.to_path_buf());
    }

    let mut search_roots = Vec::with_capacity(5);
    if let Some(models_dir) = user_models_dir() {
        search_roots.push(models_dir);
    }
    search_roots.push(PathBuf::from("/usr/share/waylan/models"));
    search_roots.push(PathBuf::from("models"));
    search_roots.push(PathBuf::from("../models"));
    search_roots.push(PathBuf::from("../../models"));

    for root in &search_roots {
        for sub in subpaths {
            let candidate = root.join(sub);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 获取用户默认配置路径 (~/.config/waylan/config.toml)
pub fn config_path() -> Option<PathBuf> {
    if let Some(custom_path) = env_var_non_empty("WAYLAN_CONFIG_PATH") {
        return Some(PathBuf::from(custom_path));
    }
    if let Some(cfg_dir) = user_config_dir() {
        return Some(cfg_dir.join("config.toml"));
    }
    Some(fallback_runtime_dir().join("config.toml"))
}

/// 获取符合 XDG 规范的用户词库文件存储路径 (~/.local/share/waylan/user_dict.bin)
pub fn user_dict_path() -> PathBuf {
    if let Some(custom) = env_var_non_empty("WAYLAN_USER_DICT") {
        return PathBuf::from(custom);
    }
    user_data_dir().join("user_dict.bin")
}

/// 获取默认系统基础词库的多级查找路径（环境变量 -> 用户数据目录 -> 系统公共目录 -> 相对路径）
pub fn base_dict_search_paths() -> Vec<PathBuf> {
    data_file_search_paths(Some("WAYLAN_DICT_PATH"), "base.dict.bin")
}

/// 获取符合 XDG 规范的用户辅助与混输词库存储路径 (~/.local/share/waylan/aux.dict.bin)
pub fn user_aux_dict_path() -> PathBuf {
    if let Some(custom) = env_var_non_empty("WAYLAN_AUX_DICT_PATH") {
        return PathBuf::from(custom);
    }
    user_data_dir().join("aux.dict.bin")
}

/// 获取辅助与混输词库（英文与符号表情）的多级查找路径
pub fn aux_dict_search_paths() -> Vec<PathBuf> {
    let mut paths = data_file_search_paths(None, "aux.dict.bin");
    let user_aux = user_aux_dict_path();
    if !paths.contains(&user_aux) {
        paths.insert(0, user_aux);
    }
    paths
}

/// 获取简繁转换词库（STPhrases + STCharacters）的多级查找路径
pub fn opencc_dict_search_paths() -> Vec<PathBuf> {
    data_file_search_paths(Some("WAYLAN_OPENCC_DICT_PATH"), "opencc.dict.bin")
}
