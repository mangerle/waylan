use crate::types::{AlgoCandidate, CandidateTier};

/// 剪贴板历史管理器 (Clipboard Manager)
///
/// # 设计原理
/// - **实现初衷**：1:1 对标微信键盘的剪贴板快捷候选提示。
/// - **核心优势**：在用户敲入 `v`、`zt` 或 `niantie` 时直接将最近复制的内容作为高优先级候选呈现，支持极速一键粘贴。
/// - **代价与局限**：内存中仅保存最近 10 条剪贴记录，单条内容超长时展示缩略预览。
#[derive(Debug, Default, Clone)]
pub struct ClipboardManager {
    /// 最近剪贴板历史（最新项位于头部）
    history: Vec<String>,
}

impl ClipboardManager {
    pub fn new() -> Self {
        Self {
            history: Vec::with_capacity(10),
        }
    }

    /// 记录一条新的剪贴板内容
    pub fn record_copy(&mut self, text: impl Into<String>) {
        let val = text.into();
        let trimmed = val.trim();
        if trimmed.is_empty() {
            return;
        }

        // 去重移至头部
        self.history.retain(|item| item != trimmed);
        self.history.insert(0, trimmed.to_string());
        if self.history.len() > 10 {
            self.history.truncate(10);
        }
    }

    /// 获取最新的剪贴板文本
    pub fn latest(&self) -> Option<&str> {
        self.history.first().map(|s| s.as_str())
    }

    /// 尝试根据输入生成剪贴板候选词
    pub fn try_eval_candidates(&self, input: &str) -> Vec<AlgoCandidate> {
        let mut candidates = Vec::new();
        let clean = input.to_lowercase();

        // 当用户敲入 "v"（且未继续输入算术表达式）、"zt"（粘贴简拼）或 "niantie" 时触发提示
        let is_paste_trigger = clean == "v" || clean == "zt" || clean == "niantie";
        if !is_paste_trigger {
            return candidates;
        }

        for (idx, item) in self.history.iter().take(3).enumerate() {
            let preview = if item.chars().count() > 18 {
                let short: String = item.chars().take(18).collect();
                format!("{}...", short)
            } else {
                item.clone()
            };

            let comment = if idx == 0 {
                "[剪贴板:最新]".to_string()
            } else {
                format!("[剪贴板:{}]", idx + 1)
            };

            candidates.push(
                AlgoCandidate::new(item.clone(), input, 999_999 - (idx as u32 * 1_000))
                    .with_tier(CandidateTier::ExactWord)
                    .with_comment(format!("{} {}", comment, preview)),
            );
        }

        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_record_and_candidate() {
        let mut manager = ClipboardManager::new();
        manager.record_copy("https://waylan.org");
        manager.record_copy("13800138000");

        assert_eq!(manager.latest(), Some("13800138000"));

        let cands_v = manager.try_eval_candidates("v");
        assert!(!cands_v.is_empty());
        assert_eq!(cands_v[0].text, "13800138000");
        assert_eq!(cands_v[1].text, "https://waylan.org");

        let cands_zt = manager.try_eval_candidates("zt");
        assert!(!cands_zt.is_empty());
        assert_eq!(cands_zt[0].text, "13800138000");
    }
}
