//! 可序列化的语言模型与候选排序特征权重结构体
//!
//! # 设计原理
//! - **实现初衷**：将原本硬编码在束搜索解码器、`BigramLanguageModel` 与候选排序流水线中的魔法常数
//!   （切词惩罚、词长奖励、半成品扣分、纠错代价惩罚、用户热度倍率）抽离为单一可序列化参数对象，
//!   使离线训练（`train_weights_softmax`）输出的权重可直接挂载到线上引擎，形成完整的量化闭环。
//! - **核心优势**：所有字段带 `#[serde(default)]` 与合理的经验证默认值，老配置文件零迁移成本；
//!   可随时在 `eval_bench` 中做交叉熵训练拟合与回归对比。
//! - **代价与局限**：线性加权模型无法表达高阶非线性特征交互。

use serde::{Deserialize, Serialize};

/// 语言模型与候选排序可训练权重参数
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoringWeights {
    /// 切词惩罚基准（每次词转移扣除的基础代价，默认 18_000）
    pub base_word_penalty: i64,
    /// 多字词长度奖励系数（每多一个汉字增加的得分，默认 16_000）
    pub length_bonus_per_char: i64,
    /// 尾部半成品补全惩罚扣分（防止半成品反超精确整词，默认 50）
    pub partial_extension_penalty: u32,
    /// 纠错路径代价缩放倍率（按切分路径代价乘以该系数扣分，默认 77）
    pub typo_cost_penalty: u32,
    /// 用户自适应词频提权缩放倍率（千分比，1000 表示 1.0x，默认 1000）
    pub user_boost_scale_permille: u32,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            base_word_penalty: 18_000,
            length_bonus_per_char: 16_000,
            partial_extension_penalty: 50,
            typo_cost_penalty: 77,
            user_boost_scale_permille: 1_000,
        }
    }
}

impl ScoringWeights {
    /// 序列化为 `key = value` 文本格式（无需额外 JSON/TOML 依赖即可读写权重文件）
    pub fn to_kv_string(&self) -> String {
        format!(
            "base_word_penalty = {}\nlength_bonus_per_char = {}\npartial_extension_penalty = {}\ntypo_cost_penalty = {}\nuser_boost_scale_permille = {}\n",
            self.base_word_penalty,
            self.length_bonus_per_char,
            self.partial_extension_penalty,
            self.typo_cost_penalty,
            self.user_boost_scale_permille
        )
    }

    /// 从 `key = value` 文本格式反序列化权重配置（未声明字段回落至默认值）
    ///
    /// # Errors
    /// 当权重字段的值无法解析为对应整数类型时返回错误描述。
    pub fn from_kv_str(content: &str) -> Result<Self, String> {
        let mut weights = Self::default();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((key, val)) = trimmed.split_once('=') else {
                continue;
            };
            let val = val.trim();
            match key.trim() {
                "base_word_penalty" => {
                    weights.base_word_penalty = val
                        .parse()
                        .map_err(|e| format!("解析 base_word_penalty 失败: {e}"))?;
                }
                "length_bonus_per_char" => {
                    weights.length_bonus_per_char = val
                        .parse()
                        .map_err(|e| format!("解析 length_bonus_per_char 失败: {e}"))?;
                }
                "partial_extension_penalty" => {
                    weights.partial_extension_penalty = val
                        .parse()
                        .map_err(|e| format!("解析 partial_extension_penalty 失败: {e}"))?;
                }
                "typo_cost_penalty" => {
                    weights.typo_cost_penalty = val
                        .parse()
                        .map_err(|e| format!("解析 typo_cost_penalty 失败: {e}"))?;
                }
                "user_boost_scale_permille" => {
                    weights.user_boost_scale_permille = val
                        .parse()
                        .map_err(|e| format!("解析 user_boost_scale_permille 失败: {e}"))?;
                }
                _ => {}
            }
        }
        Ok(weights)
    }
}
