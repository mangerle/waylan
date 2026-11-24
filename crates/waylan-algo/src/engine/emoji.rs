//! 打字时实时 Emoji 推荐引擎
//!
//! # 设计原理
//! - **实现初衷**：参考现代商业输入法体验，当用户输入包含情绪、动作、物品或常用符号的词句时，
//!   自动在第 3 候选位推荐最贴合的 Emoji，兼顾流畅输入与表达趣味性。
//! - **核心优势**：采用编译期静态常量映射与 `OnceLock` 哈希索引，O(1) 零分配查表，
//!   同时支持按候选词文本（如“开心”、“加油”）及纯拼音（如 `kaixin`）双重匹配；
//!   不打扰第 1、2 位高置信度文字候选。
//! - **代价与局限**：仅覆盖高频日常词汇，且插入固定在第 3 位（下标 2）；可通过 `AlgoConfig::enable_emoji_suggest` 关闭。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::types::{AlgoCandidate, CandidateTier};

/// 高频中文词与对应 Emoji（使用 Unicode 转义表示以保持源码纯 ASCII/中文洁净）
static WORD_EMOJI_PAIRS: &[(&str, &str)] = &[
    ("开心", "\u{1F60A}"),
    ("高兴", "\u{1F604}"),
    ("快乐", "\u{1F973}"),
    ("笑", "\u{1F602}"),
    ("哈哈", "\u{1F602}"),
    ("大笑", "\u{1F923}"),
    ("微笑", "\u{1F642}"),
    ("难过", "\u{1F622}"),
    ("伤心", "\u{1F62D}"),
    ("哭", "\u{1F62D}"),
    ("流泪", "\u{1F622}"),
    ("生气", "\u{1F621}"),
    ("愤怒", "\u{1F624}"),
    ("惊讶", "\u{1F632}"),
    ("震惊", "\u{1F631}"),
    ("害怕", "\u{1F628}"),
    ("害羞", "\u{1F633}"),
    ("困", "\u{1F634}"),
    ("睡觉", "\u{1F634}"),
    ("晚安", "\u{1F319}"),
    ("早安", "\u{2600}\u{FE0F}"),
    ("再见", "\u{1F44B}"),
    ("拜拜", "\u{1F44B}"),
    ("谢谢", "\u{1F64F}"),
    ("感谢", "\u{1F64F}"),
    ("拜托", "\u{1F64F}"),
    ("喜欢", "\u{2764}\u{FE0F}"),
    ("爱你", "\u{2764}\u{FE0F}"),
    ("爱心", "\u{2764}\u{FE0F}"),
    ("比心", "\u{1FAF6}"),
    ("心碎", "\u{1F494}"),
    ("赞", "\u{1F44D}"),
    ("点赞", "\u{1F44D}"),
    ("厉害", "\u{1F44D}"),
    ("优秀", "\u{1F31F}"),
    ("加油", "\u{1F4AA}"),
    ("努力", "\u{1F4AA}"),
    ("鼓掌", "\u{1F44F}"),
    ("握手", "\u{1F91D}"),
    ("庆祝", "\u{1F389}"),
    ("恭喜", "\u{1F389}"),
    ("生日", "\u{1F382}"),
    ("蛋糕", "\u{1F382}"),
    ("礼物", "\u{1F381}"),
    ("干杯", "\u{1F37B}"),
    ("太阳", "\u{2600}\u{FE0F}"),
    ("晴天", "\u{2600}\u{FE0F}"),
    ("下雨", "\u{1F327}\u{FE0F}"),
    ("下雪", "\u{2744}\u{FE0F}"),
    ("星星", "\u{2B50}"),
    ("月亮", "\u{1F319}"),
    ("彩虹", "\u{1F308}"),
    ("火", "\u{1F525}"),
    ("热门", "\u{1F525}"),
    ("咖啡", "\u{2615}"),
    ("奶茶", "\u{1F9CB}"),
    ("吃饭", "\u{1F35A}"),
    ("苹果", "\u{1F34E}"),
    ("西瓜", "\u{1F349}"),
    ("猫", "\u{1F431}"),
    ("猫咪", "\u{1F431}"),
    ("狗", "\u{1F436}"),
    ("狗狗", "\u{1F436}"),
    ("火箭", "\u{1F680}"),
    ("飞机", "\u{2708}\u{FE0F}"),
    ("汽车", "\u{1F697}"),
    ("钱", "\u{1F4B0}"),
    ("发财", "\u{1F4B0}"),
    ("满分", "\u{1F4AF}"),
    ("思考", "\u{1F914}"),
    ("疑问", "\u{2753}"),
    ("问题", "\u{2753}"),
    ("完成", "\u{2705}"),
    ("成功", "\u{2705}"),
    ("失败", "\u{274C}"),
    ("警告", "\u{26A0}\u{FE0F}"),
    ("音乐", "\u{1F3B5}"),
];

/// 高频无隔音符拼音与对应 Emoji 兜底映射表
static PINYIN_EMOJI_PAIRS: &[(&str, &str)] = &[
    ("kaixin", "\u{1F60A}"),
    ("gaoxing", "\u{1F604}"),
    ("haha", "\u{1F602}"),
    ("nanguo", "\u{1F622}"),
    ("jiayou", "\u{1F4AA}"),
    ("xiexie", "\u{1F64F}"),
    ("baibai", "\u{1F44B}"),
    ("zaijian", "\u{1F44B}"),
    ("wanan", "\u{1F319}"),
    ("zaoan", "\u{2600}\u{FE0F}"),
    ("xihuan", "\u{2764}\u{FE0F}"),
    ("aini", "\u{2764}\u{FE0F}"),
    ("dianzan", "\u{1F44D}"),
    ("shengri", "\u{1F382}"),
    ("gongxi", "\u{1F389}"),
    ("qingzhu", "\u{1F389}"),
    ("kafei", "\u{2615}"),
    ("naicha", "\u{1F9CB}"),
    ("huojian", "\u{1F680}"),
    ("sikao", "\u{1F914}"),
];

fn word_emoji_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| WORD_EMOJI_PAIRS.iter().copied().collect())
}

fn pinyin_emoji_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| PINYIN_EMOJI_PAIRS.iter().copied().collect())
}

/// 根据当前前两位候选文本与原始拼音检索最匹配的实时推荐 Emoji 候选
///
/// 命中后返回一个 `consumed_len == raw_input.len()` 的候选，若现有候选中已包含该表情则返回 `None`。
pub(crate) fn suggest_emoji_candidate(
    raw_input: &str,
    candidates: &[AlgoCandidate],
) -> Option<AlgoCandidate> {
    let wmap = word_emoji_map();
    let emoji = candidates
        .iter()
        .take(2)
        .find_map(|c| wmap.get(c.text.as_str()).copied())
        .or_else(|| pinyin_emoji_map().get(raw_input).copied())?;

    if candidates.iter().any(|c| c.text == emoji) {
        return None;
    }

    let mut cand =
        AlgoCandidate::new(emoji, raw_input, 1).with_tier(CandidateTier::SentenceViterbi);
    cand.consumed_len = Some(raw_input.len());
    Some(cand)
}
