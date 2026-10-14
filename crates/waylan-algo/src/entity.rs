use std::collections::HashMap;

/// 专有名词与命名实体保护管理器 (NER Protection Engine)
///
/// # 设计原理
/// - **实现初衷**：防止技术名词、专有名词、品牌缩写（如 rust, linux, wayland, viterbi 等）
///   在音节切分阶段被贪心或回溯切分器强制拆碎成毫无意义的拼音残片。
/// - **核心优势**：在 Prism 阶段将实体识别为原子不可分割的切分块（Atomic Chunk），以零代价优先放行；
///   并在候选词输出管道中以最高优先级保真推荐原始标准大小写词形。
#[derive(Debug, Clone)]
pub struct EntityProtectionManager {
    /// 受保护的专有名词与技术实体映射（小写键 -> 原始标准大小写展示词形）
    entities: HashMap<String, String>,
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
            entities: HashMap::with_capacity(48),
        };
        mgr.load_builtin_entities();
        mgr
    }

    /// 载入内置核心技术专有名词与重要实体
    fn load_builtin_entities(&mut self) {
        let default_entities = [
            // 核心开发工具与语言
            "rust",
            "cargo",
            "linux",
            "wayland",
            "waylan",
            "viterbi",
            "prism",
            "docker",
            "GitHub",
            "GitLab",
            "tokio",
            "egui",
            "sqlite",
            "bincode",
            "python",
            "golang",
            "cplusplus",
            "wasm",
            "cmake",
            // 现代 AI 与前沿实体
            "DeepSeek",
            "Gemini",
            "ChatGPT",
            "OpenAI",
            "Claude",
            "Transformer",
            "Llama",
            // 系统与图形桌面实体
            "ubuntu",
            "debian",
            "archlinux",
            "hyprland",
            "sway",
            "gnome",
            "kde",
            // 中文专有品牌与项目
            "微澜输入法",
            "清华大学",
            "北京大学",
        ];
        for &e in &default_entities {
            self.entities.insert(e.to_lowercase(), e.to_string());
        }
    }

    /// 动态注册新的受保护专有名词/实体（保留原始大小写词形）
    pub fn register_entity(&mut self, entity: &str) {
        let trimmed = entity.trim();
        if !trimmed.is_empty() {
            self.entities
                .insert(trimmed.to_lowercase(), trimmed.to_string());
        }
    }

    /// 检查输入字符串是否完全匹配受保护实体，命中时返回保真的标准大小写词形
    pub fn match_exact(&self, input: &str) -> Option<String> {
        if let Some(canonical) = self.entities.get(input) {
            return Some(canonical.clone());
        }
        let lower = input.to_lowercase();
        self.entities.get(&lower).cloned()
    }

    /// 检查输入字符串前缀是否匹配受保护实体（前向最大匹配，严格沿 UTF-8 字符边界切分）
    pub fn match_prefix(&self, input: &str) -> Option<(String, usize)> {
        let mut boundaries: Vec<usize> = Vec::with_capacity(input.len());
        for (idx, ch) in input.char_indices() {
            let end = idx + ch.len_utf8();
            if end >= 2 {
                boundaries.push(end);
            }
        }

        for &len in boundaries.iter().rev() {
            let slice = &input[..len];
            if let Some(canonical) = self.entities.get(slice) {
                return Some((canonical.clone(), len));
            }
            let lower = slice.to_lowercase();
            if let Some(canonical) = self.entities.get(&lower) {
                return Some((canonical.clone(), len));
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

        if let Some((entity, len)) = mgr.match_prefix("rustaceans") {
            assert_eq!(entity, "rust");
            assert_eq!(len, 4);
        } else {
            panic!("预期命中前缀保护实体 rustaceans -> rust");
        }
    }

    #[test]
    fn test_custom_entity_registration_and_case_preservation() {
        let mut mgr = EntityProtectionManager::new();
        mgr.register_entity("DeepMind");
        assert_eq!(mgr.match_exact("deepmind").as_deref(), Some("DeepMind"));
        assert_eq!(mgr.match_exact("DEEPMIND").as_deref(), Some("DeepMind"));
        assert_eq!(mgr.match_exact("github").as_deref(), Some("GitHub"));
        assert_eq!(mgr.match_exact("chatgpt").as_deref(), Some("ChatGPT"));

        if let Some((entity, len)) = mgr.match_prefix("chatgpt4o") {
            assert_eq!(entity, "ChatGPT");
            assert_eq!(len, 7);
        } else {
            panic!("预期命中前缀保护实体 chatgpt4o -> ChatGPT");
        }
    }

    #[test]
    fn test_multibyte_utf8_prefix_match_no_panic() {
        let mgr = EntityProtectionManager::new();
        // 包含中文字符但未命中的前缀不应触发字节边界切片 Panic
        assert!(mgr.match_prefix("你好世界abc").is_none());
        // 包含中文字符且命中内置中文实体的前缀应返回正确词形与字节长度
        let input = "微澜输入法引擎";
        let matched = mgr.match_prefix(input);
        assert_eq!(
            matched,
            Some(("微澜输入法".to_string(), "微澜输入法".len()))
        );
    }
}
