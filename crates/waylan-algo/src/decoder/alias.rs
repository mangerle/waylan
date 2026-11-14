//! 拼音习惯音与叹词音别名衍生映射规则模块
//!
//! # 设计原理
//! - **实现初衷**：在现代汉语拼音方案中，存在特殊的单纯辅音鼻音叹词（如“嗯”的注音为 `n`/`ng`，“呣/呒”的注音为 `m`），
//!   以及日常口语俗读习惯音（如打 `en` 想输入“嗯”，打 `me`/`mu` 想输入“呣”）。
//! - **核心优势**：单字与连打词组（如 `enen` 出“嗯嗯”、`enhao` 出“嗯好”）统一级联生效，扩展新规则零算法侵入。
//! - **代价与局限**：在音节格生成中增加少量确定性衍生边，但由 FSA 前缀守卫阻断无谓扩散。

/// 拼音习惯音与叹词音衍生别名规则
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HabitualAliasRule {
    /// 用户的日常习惯输入拼音切片（如 "en", "me", "mu"）
    pub alias: &'static str,
    /// 对应的底层规范音节（如 "n", "ng", "m"）
    pub targets: &'static [&'static str],
    /// 衍生边代价惩罚（适度微弱惩罚以确保标准全拼字如“恩”默认优先展示）
    pub cost_penalty: u32,
}

/// 全局静态拼音习惯音与叹词音别名映射表
pub static HABITUAL_ALIAS_RULES: &[HabitualAliasRule] = &[
    HabitualAliasRule {
        alias: "en",
        targets: &["n", "ng"],
        cost_penalty: 3,
    },
    HabitualAliasRule {
        alias: "me",
        targets: &["m"],
        cost_penalty: 3,
    },
    HabitualAliasRule {
        alias: "mu",
        targets: &["m"],
        cost_penalty: 3,
    },
    HabitualAliasRule {
        alias: "heng",
        targets: &["hng"],
        cost_penalty: 3,
    },
];

/// 查找输入拼音切片对应的习惯音别名衍生目标音节与对应惩罚值
#[inline]
pub fn get_habitual_alias_targets(slice: &str) -> Option<(&'static [&'static str], u32)> {
    HABITUAL_ALIAS_RULES
        .iter()
        .find(|r| r.alias == slice)
        .map(|r| (r.targets, r.cost_penalty))
}

/// 判断给定的目标音节是否属于由习惯音别名衍生得到的音节
#[inline]
pub fn is_habitual_alias_target(target: &str) -> bool {
    HABITUAL_ALIAS_RULES
        .iter()
        .any(|r| r.targets.contains(&target))
}

/// 判定指定的拼音查询键是否包含由习惯音别名衍生的音节
#[inline]
pub fn is_habitual_alias_key(key: &str) -> bool {
    key.split('\'').any(is_habitual_alias_target)
}

/// 校准习惯音别名候选的得分
#[inline]
pub fn calibrate_habitual_alias_score(score: u32, is_single_char: bool) -> u32 {
    if is_single_char {
        ((score as f64) * 0.035) as u32
    } else {
        ((score as f64) * 0.8) as u32
    }
}
