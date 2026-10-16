//! 配置文件与词库资产的多级路径探测与环境感知

use std::env;
use std::fs;
use std::path::PathBuf;

/// 读取非空环境变量（自动过滤空白字符串与未设置变量）
pub fn env_var_non_empty(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// 辅助根据执行文件所在目录向多级父路径推入候选词库位置（适应开发、测试与各类宿主子进程环境）
fn push_exe_relative_candidates(paths: &mut Vec<PathBuf>, rel_subpath: &str) {
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            paths.push(parent.join(rel_subpath));
            paths.push(parent.join("..").join(rel_subpath));
            paths.push(parent.join("../..").join(rel_subpath));
            paths.push(parent.join("../../..").join(rel_subpath));
            paths.push(parent.join("../share/waylan").join(rel_subpath));
        }
    }
}

/// 获取无 HOME 环境下的受控运行时回退目录
/// 优先使用 XDG_RUNTIME_DIR/waylan，其次回退到带有当前用户 UID 标识且权限为 0700 的 /tmp/waylan-<uid>
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

/// 获取用户默认配置路径 (~/.config/waylan/config.toml)
pub fn config_path() -> Option<PathBuf> {
    if let Some(custom_path) = env_var_non_empty("WAYLAN_CONFIG_PATH") {
        return Some(PathBuf::from(custom_path));
    }
    if let Some(config_home) = env_var_non_empty("XDG_CONFIG_HOME") {
        return Some(
            PathBuf::from(config_home)
                .join("waylan")
                .join("config.toml"),
        );
    }
    if let Some(home) = env_var_non_empty("HOME") {
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("waylan")
                .join("config.toml"),
        );
    }
    Some(fallback_runtime_dir().join("config.toml"))
}

/// 获取符合 XDG 规范的用户词库文件存储路径 (~/.local/share/waylan/user_dict.bin)
pub fn user_dict_path() -> PathBuf {
    if let Some(custom) = env_var_non_empty("WAYLAN_USER_DICT") {
        return PathBuf::from(custom);
    }
    if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
        return PathBuf::from(data_home)
            .join("waylan")
            .join("user_dict.bin");
    }
    if let Some(home) = env_var_non_empty("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("waylan")
            .join("user_dict.bin");
    }
    fallback_runtime_dir().join("user_dict.bin")
}

/// 获取默认系统基础词库的多级查找路径（环境变量 -> 用户数据目录 -> 系统公共目录 -> 相对路径）
pub fn base_dict_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(16);
    if let Some(custom) = env_var_non_empty("WAYLAN_DICT_PATH") {
        paths.push(PathBuf::from(custom));
    }
    if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
        paths.push(
            PathBuf::from(data_home)
                .join("waylan")
                .join("base.dict.bin"),
        );
    }
    if let Some(home) = env_var_non_empty("HOME") {
        paths.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("waylan")
                .join("base.dict.bin"),
        );
    }
    paths.push(PathBuf::from("/usr/share/waylan/data/base.dict.bin"));
    paths.push(PathBuf::from("/usr/share/waylan/base.dict.bin"));
    paths.push(PathBuf::from("/usr/local/share/waylan/data/base.dict.bin"));
    paths.push(PathBuf::from("/usr/local/share/waylan/base.dict.bin"));
    push_exe_relative_candidates(&mut paths, "data/base.dict.bin");
    push_exe_relative_candidates(&mut paths, "base.dict.bin");
    paths.push(PathBuf::from("data/base.dict.bin"));
    paths.push(PathBuf::from("base.dict.bin"));
    paths
}

/// 获取符合 XDG 规范的用户辅助与混输词库存储路径 (~/.local/share/waylan/aux.dict.bin)
pub fn user_aux_dict_path() -> PathBuf {
    if let Some(custom) = env_var_non_empty("WAYLAN_AUX_DICT_PATH") {
        return PathBuf::from(custom);
    }
    if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join("waylan").join("aux.dict.bin");
    }
    if let Some(home) = env_var_non_empty("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("waylan")
            .join("aux.dict.bin");
    }
    fallback_runtime_dir().join("aux.dict.bin")
}

/// 获取辅助与混输词库（英文与符号表情）的多级查找路径
pub fn aux_dict_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(16);
    paths.push(user_aux_dict_path());
    paths.push(PathBuf::from("/usr/share/waylan/data/aux.dict.bin"));
    paths.push(PathBuf::from("/usr/share/waylan/aux.dict.bin"));
    paths.push(PathBuf::from("/usr/local/share/waylan/data/aux.dict.bin"));
    paths.push(PathBuf::from("/usr/local/share/waylan/aux.dict.bin"));
    push_exe_relative_candidates(&mut paths, "data/aux.dict.bin");
    push_exe_relative_candidates(&mut paths, "aux.dict.bin");
    paths.push(PathBuf::from("data/aux.dict.bin"));
    paths.push(PathBuf::from("aux.dict.bin"));
    paths
}

/// 获取简繁转换词库（STPhrases + STCharacters）的多级查找路径
pub fn opencc_dict_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(16);
    if let Some(custom) = env_var_non_empty("WAYLAN_OPENCC_DICT_PATH") {
        paths.push(PathBuf::from(custom));
    }
    if let Some(data_home) = env_var_non_empty("XDG_DATA_HOME") {
        paths.push(
            PathBuf::from(data_home)
                .join("waylan")
                .join("opencc.dict.bin"),
        );
    }
    if let Some(home) = env_var_non_empty("HOME") {
        paths.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("waylan")
                .join("opencc.dict.bin"),
        );
    }
    paths.push(PathBuf::from("/usr/share/waylan/data/opencc.dict.bin"));
    paths.push(PathBuf::from("/usr/share/waylan/opencc.dict.bin"));
    paths.push(PathBuf::from(
        "/usr/local/share/waylan/data/opencc.dict.bin",
    ));
    paths.push(PathBuf::from("/usr/local/share/waylan/opencc.dict.bin"));
    push_exe_relative_candidates(&mut paths, "data/opencc.dict.bin");
    push_exe_relative_candidates(&mut paths, "opencc.dict.bin");
    paths.push(PathBuf::from("data/opencc.dict.bin"));
    paths.push(PathBuf::from("opencc.dict.bin"));
    paths
}
