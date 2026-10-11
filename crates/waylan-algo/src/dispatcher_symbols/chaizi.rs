use crate::types::AlgoCandidate;

/// U 模式：部首、叠字与拆字反查规则项
struct ChaiziEntry {
    code: &'static str,
    text: &'static str,
    comment: &'static str,
}

/// 预置 U 模式丰富拆字与叠字词库表
///
/// # 设计原理
/// - **实现初衷**：为生僻汉字与拼字（如双叠、三叠、四叠、左右结构拆字及笔画反查）提供辅助输入能力。
/// - **核心优势**：静态表紧凑常驻，零额外内存分配与反序列化开销。
/// - **代价与局限**：覆盖集合有限，适合作为常用难字与叠字的辅助查询。
const CHAIZI_TABLE: &[ChaiziEntry] = &[
    // 双叠与特殊拼字
    ChaiziEntry {
        code: "hehe",
        text: "囍",
        comment: "拆字: 喜+喜",
    },
    ChaiziEntry {
        code: "mumu",
        text: "林",
        comment: "拆字: 木+木",
    },
    ChaiziEntry {
        code: "koukou",
        text: "回",
        comment: "拆字: 口+口",
    },
    ChaiziEntry {
        code: "koukou",
        text: "吅",
        comment: "拆字: 口+口",
    },
    ChaiziEntry {
        code: "riri",
        text: "昍",
        comment: "拆字: 日+日",
    },
    ChaiziEntry {
        code: "sun",
        text: "昍",
        comment: "拆字: 日+日",
    },
    ChaiziEntry {
        code: "renren",
        text: "从",
        comment: "拆字: 人+人",
    },
    ChaiziEntry {
        code: "yueyue",
        text: "朋",
        comment: "拆字: 月+月",
    },
    ChaiziEntry {
        code: "tutu",
        text: "圭",
        comment: "拆字: 土+土",
    },
    ChaiziEntry {
        code: "huohuo",
        text: "炎",
        comment: "拆字: 火+火",
    },
    // 三叠字（生僻与典型叠字）
    ChaiziEntry {
        code: "mux3",
        text: "森",
        comment: "拆字: 木x3",
    },
    ChaiziEntry {
        code: "mumumu",
        text: "森",
        comment: "拆字: 木x3",
    },
    ChaiziEntry {
        code: "koux3",
        text: "品",
        comment: "拆字: 口x3",
    },
    ChaiziEntry {
        code: "koukoukou",
        text: "品",
        comment: "拆字: 口x3",
    },
    ChaiziEntry {
        code: "yux3",
        text: "鱻",
        comment: "拆字: 鱼x3",
    },
    ChaiziEntry {
        code: "yuyuyu",
        text: "鱻",
        comment: "拆字: 鱼x3",
    },
    ChaiziEntry {
        code: "xian",
        text: "鱻",
        comment: "拆字: 鱼x3 (xiān)",
    },
    ChaiziEntry {
        code: "niux3",
        text: "犇",
        comment: "拆字: 牛x3",
    },
    ChaiziEntry {
        code: "niuniuniu",
        text: "犇",
        comment: "拆字: 牛x3",
    },
    ChaiziEntry {
        code: "niu",
        text: "犇",
        comment: "拆字: 牛x3 (bēn)",
    },
    ChaiziEntry {
        code: "yangx3",
        text: "羴",
        comment: "拆字: 羊x3",
    },
    ChaiziEntry {
        code: "yangyangyang",
        text: "羴",
        comment: "拆字: 羊x3",
    },
    ChaiziEntry {
        code: "yang",
        text: "羴",
        comment: "拆字: 羊x3 (shān)",
    },
    ChaiziEntry {
        code: "huox3",
        text: "焱",
        comment: "拆字: 火x3",
    },
    ChaiziEntry {
        code: "huohuohuo",
        text: "焱",
        comment: "拆字: 火x3",
    },
    ChaiziEntry {
        code: "huo",
        text: "焱",
        comment: "拆字: 火x3 (yàn)",
    },
    ChaiziEntry {
        code: "jinx3",
        text: "鑫",
        comment: "拆字: 金x3",
    },
    ChaiziEntry {
        code: "jinjinjin",
        text: "鑫",
        comment: "拆字: 金x3",
    },
    ChaiziEntry {
        code: "jin",
        text: "鑫",
        comment: "拆字: 金x3 (xīn)",
    },
    ChaiziEntry {
        code: "tux3",
        text: "垚",
        comment: "拆字: 土x3",
    },
    ChaiziEntry {
        code: "tututu",
        text: "垚",
        comment: "拆字: 土x3",
    },
    ChaiziEntry {
        code: "tu",
        text: "垚",
        comment: "拆字: 土x3 (yáo)",
    },
    ChaiziEntry {
        code: "shuix3",
        text: "淼",
        comment: "拆字: 水x3",
    },
    ChaiziEntry {
        code: "shuishuishui",
        text: "淼",
        comment: "拆字: 水x3",
    },
    ChaiziEntry {
        code: "shui",
        text: "淼",
        comment: "拆字: 水x3 (miǎo)",
    },
    ChaiziEntry {
        code: "chex3",
        text: "轰",
        comment: "拆字: 车x3",
    },
    ChaiziEntry {
        code: "checheche",
        text: "轰",
        comment: "拆字: 车x3",
    },
    ChaiziEntry {
        code: "che",
        text: "轰",
        comment: "拆字: 车x3 (hōng)",
    },
    ChaiziEntry {
        code: "renx3",
        text: "众",
        comment: "拆字: 人x3",
    },
    ChaiziEntry {
        code: "renrenren",
        text: "众",
        comment: "拆字: 人x3",
    },
    ChaiziEntry {
        code: "shix3",
        text: "磊",
        comment: "拆字: 石x3",
    },
    ChaiziEntry {
        code: "shishishi",
        text: "磊",
        comment: "拆字: 石x3",
    },
    ChaiziEntry {
        code: "rix3",
        text: "晶",
        comment: "拆字: 日x3",
    },
    ChaiziEntry {
        code: "ririri",
        text: "晶",
        comment: "拆字: 日x3",
    },
    ChaiziEntry {
        code: "tianx3",
        text: "畾",
        comment: "拆字: 田x3",
    },
    ChaiziEntry {
        code: "tiantiantian",
        text: "畾",
        comment: "拆字: 田x3",
    },
    ChaiziEntry {
        code: "baix3",
        text: "皛",
        comment: "拆字: 白x3",
    },
    ChaiziEntry {
        code: "baibaibai",
        text: "皛",
        comment: "拆字: 白x3",
    },
    ChaiziEntry {
        code: "max3",
        text: "骉",
        comment: "拆字: 马x3",
    },
    ChaiziEntry {
        code: "mamama",
        text: "骉",
        comment: "拆字: 马x3",
    },
    // 四叠字
    ChaiziEntry {
        code: "rrrr",
        text: "叕",
        comment: "拆字: 又x4",
    },
    ChaiziEntry {
        code: "youx4",
        text: "叕",
        comment: "拆字: 又x4 (zhuó)",
    },
    ChaiziEntry {
        code: "huox4",
        text: "燚",
        comment: "拆字: 火x4 (yì)",
    },
    // 左右与上下结构常见拆字组合
    ChaiziEntry {
        code: "riyue",
        text: "明",
        comment: "拆字: 日+月",
    },
    ChaiziEntry {
        code: "nuzi",
        text: "好",
        comment: "拆字: 女+子",
    },
    ChaiziEntry {
        code: "xiaotu",
        text: "尘",
        comment: "拆字: 小+土",
    },
    ChaiziEntry {
        code: "buyi",
        text: "歪",
        comment: "拆字: 不+正",
    },
    ChaiziEntry {
        code: "tianli",
        text: "男",
        comment: "拆字: 田+力",
    },
    ChaiziEntry {
        code: "shancai",
        text: "岳",
        comment: "拆字: 丘+山",
    },
    ChaiziEntry {
        code: "renwu",
        text: "件",
        comment: "拆字: 人+牛",
    },
    ChaiziEntry {
        code: "mufan",
        text: "板",
        comment: "拆字: 木+反",
    },
    // 基础笔画与笔画组合代码
    ChaiziEntry {
        code: "h",
        text: "一",
        comment: "笔画: 横(h)",
    },
    ChaiziEntry {
        code: "s",
        text: "丨",
        comment: "笔画: 竖(s)",
    },
    ChaiziEntry {
        code: "p",
        text: "丿",
        comment: "笔画: 撇(p)",
    },
    ChaiziEntry {
        code: "n",
        text: "丶",
        comment: "笔画: 捺/点(n)",
    },
    ChaiziEntry {
        code: "d",
        text: "丶",
        comment: "笔画: 点(d)",
    },
    ChaiziEntry {
        code: "z",
        text: "乙",
        comment: "笔画: 折(z)",
    },
    ChaiziEntry {
        code: "sh",
        text: "十",
        comment: "笔画: 竖+横",
    },
    ChaiziEntry {
        code: "hh",
        text: "二",
        comment: "笔画: 横+横",
    },
    ChaiziEntry {
        code: "hhh",
        text: "三",
        comment: "笔画: 横+横+横",
    },
    ChaiziEntry {
        code: "ws",
        text: "卜",
        comment: "笔画: 竖+点",
    },
    ChaiziEntry {
        code: "hspn",
        text: "札",
        comment: "笔画: 横竖撇捺",
    },
];

/// 查询 U 模式拆字与叠字候选列表
pub(crate) fn query_u_mode(input: &str) -> Vec<AlgoCandidate> {
    let mut candidates = Vec::with_capacity(8);
    for entry in CHAIZI_TABLE {
        if entry.code == input {
            candidates.push(
                AlgoCandidate::new(entry.text, format!("u{}", entry.code), 20_000)
                    .with_comment(entry.comment),
            );
        } else if entry.code.starts_with(input) {
            candidates.push(
                AlgoCandidate::new(entry.text, format!("u{}", entry.code), 10_000)
                    .with_comment(entry.comment),
            );
        }
    }
    candidates.sort_by_key(|c| std::cmp::Reverse(c.score));
    candidates
}
