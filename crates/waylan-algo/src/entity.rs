use std::collections::HashSet;

/// 专有名词与命名实体保护管理器 (NER Protection Engine)
///
/// # 设计原理
/// - **实现初衷**：防止技术名词、专有名词、品牌缩写（如 rust, linux, wayland, viterbi 等）
///   在音节切分阶段被贪心或回溯切分器强制拆碎成毫无意义的拼音残片。
/// - **核心优势**：在 Prism 阶段将实体识别为原子不可分割的切分块（Atomic Chunk），以零代价优先放行；
///   并在候选词输出管道中以最高优先级保真推荐。
#[derive(Debug, Clone)]
pub struct EntityProtectionManager {
    /// 受保护的专有名词与技术实体集合（小写键，原形展示）
    entities: HashSet<String>,
}

impl Default for EntityProtectionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityProtectionManager {
    /// 全局静态默认内置实体保护管理器单例
    pub fn global() -> &'static EntityProtectionManager {
        static INSTANCE: std::sync::OnceLock<EntityProtectionManager> = std::sync::OnceLock::new();
        INSTANCE.get_or_init(Self::new)
    }

    /// 创建带有默认核心技术专名与实体的保护管理器
    pub fn new() -> Self {
        let mut mgr = Self {
            entities: HashSet::new(),
        };
        mgr.load_builtin_entities();
        mgr
    }

    /// 载入内置核心技术专有名词与重要实体
    fn load_builtin_entities(&mut self) {
        let default_entities = [
            // 核心开发工具与语言
            "rust", "cargo", "linux", "wayland", "waylan", "viterbi", "prism",
            "docker", "github", "gitlab", "tokio", "egui", "sqlite", "bincode",
            "python", "golang", "cplusplus", "wasm", "cmake",
            // 现代 AI 与前沿实体
            "deepseek", "gemini", "chatgpt", "openai", "claude", "transformer", "llama",
            // 系统与图形桌面实体
            "ubuntu", "debian", "archlinux", "hyprland", "sway", "gnome", "kde",
            // 中文专有品牌与项目
            "微澜输入法", "微信键盘", "清华大学", "北京大学",
        ];
        for &e in &default_entities {
            self.entities.insert(e.to_lowercase());
        }
    }

    /// 动态注册新的受保护专有名词/实体
    pub fn register_entity(&mut self, entity: &str) {
        let trimmed = entity.trim();
        if !trimmed.is_empty() {
            self.entities.insert(trimmed.to_lowercase());
        }
    }

    /// 检查输入字符串是否完全匹配受保护实体
    pub fn match_exact(&self, input: &str) -> Option<String> {
        if self.entities.contains(input) {
            return Some(input.to_string());
        }
        let lower = input.to_lowercase();
        if self.entities.contains(&lower) {
            Some(lower)
        } else {
            None
        }
    }

    /// 检查输入字符串前缀是否匹配受保护实体（前向最大匹配）
    pub fn match_prefix(&self, input: &str) -> Option<(String, usize)> {
        let lower = input.to_lowercase();
        let max_len = lower.len();
        for len in (2..=max_len).rev() {
            let slice = &lower[..len];
            if self.entities.contains(slice) {
                return Some((slice.to_string(), len));
            }
        }
        None
    }

    /// 受保护实体总数
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_entities_match() {
        let mgr = EntityProtectionManager::new();
        assert!(mgr.match_exact("rust").is_some());
        assert!(mgr.match_exact("Rust").is_some());
        assert!(mgr.match_exact("WAYLAND").is_some());
        assert!(mgr.match_exact("viterbi").is_some());
        assert!(mgr.match_exact("xyz123").is_none());

        let prefix_res = mgr.match_prefix("rustaceans");
        assert!(prefix_res.is_some());
        let (entity, len) = prefix_res.unwrap();
        assert_eq!(entity, "rust");
        assert_eq!(len, 4);
    }

    #[test]
    fn test_custom_entity_registration() {
        let mut mgr = EntityProtectionManager::new();
        mgr.register_entity("DeepMind");
        assert!(mgr.match_exact("deepmind").is_some());
    }
}
