use crate::types::AlgoCandidate;

/// U 模式：部首、叠字与拆字反查规则项
struct ChaiziEntry {
    code: &'static str,
    text: &'static str,
    comment: &'static str,
}

/// 预置 U 模式丰富拆字与叠字词库表
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

/// 查询 U 模式拆字候选
pub(crate) fn query_u_mode(input: &str) -> Vec<AlgoCandidate> {
    let mut candidates = Vec::with_capacity(8);
    for entry in CHAIZI_TABLE {
        if entry.code == input || entry.code.starts_with(input) {
            candidates.push(
                AlgoCandidate::new(entry.text, format!("u{}", entry.code), 10_000)
                    .with_comment(entry.comment),
            );
        }
    }
    candidates
}

/// I 模式：根据输入查询特殊符号与序号候选
pub(crate) fn query_i_mode(input: &str) -> Vec<AlgoCandidate> {
    let mut candidates = Vec::with_capacity(8);

    // 1. 数字序号与大写金额
    match input {
        "1" => {
            push_sym(&mut candidates, "①", "i1", "带圈序号");
            push_sym(&mut candidates, "⑴", "i1", "括号序号");
            push_sym(&mut candidates, "⒈", "i1", "带点序号");
            push_sym(&mut candidates, "Ⅰ", "i1", "罗马数字");
            push_sym(&mut candidates, "ⅰ", "i1", "小写罗马");
            push_sym(&mut candidates, "壹", "i1", "大写数字");
            push_sym(&mut candidates, "第一", "i1", "中文序数");
        }
        "2" => {
            push_sym(&mut candidates, "②", "i2", "带圈序号");
            push_sym(&mut candidates, "⑵", "i2", "括号序号");
            push_sym(&mut candidates, "⒉", "i2", "带点序号");
            push_sym(&mut candidates, "Ⅱ", "i2", "罗马数字");
            push_sym(&mut candidates, "ⅱ", "i2", "小写罗马");
            push_sym(&mut candidates, "贰", "i2", "大写数字");
            push_sym(&mut candidates, "第二", "i2", "中文序数");
        }
        "3" => {
            push_sym(&mut candidates, "③", "i3", "带圈序号");
            push_sym(&mut candidates, "⑶", "i3", "括号序号");
            push_sym(&mut candidates, "⒊", "i3", "带点序号");
            push_sym(&mut candidates, "Ⅲ", "i3", "罗马数字");
            push_sym(&mut candidates, "ⅲ", "i3", "小写罗马");
            push_sym(&mut candidates, "叁", "i3", "大写数字");
            push_sym(&mut candidates, "第三", "i3", "中文序数");
        }
        "4" => {
            push_sym(&mut candidates, "④", "i4", "带圈序号");
            push_sym(&mut candidates, "⑷", "i4", "括号序号");
            push_sym(&mut candidates, "⒋", "i4", "带点序号");
            push_sym(&mut candidates, "Ⅳ", "i4", "罗马数字");
            push_sym(&mut candidates, "肆", "i4", "大写数字");
            push_sym(&mut candidates, "第四", "i4", "中文序数");
        }
        "5" => {
            push_sym(&mut candidates, "⑤", "i5", "带圈序号");
            push_sym(&mut candidates, "⑸", "i5", "括号序号");
            push_sym(&mut candidates, "⒌", "i5", "带点序号");
            push_sym(&mut candidates, "Ⅴ", "i5", "罗马数字");
            push_sym(&mut candidates, "伍", "i5", "大写数字");
            push_sym(&mut candidates, "第五", "i5", "中文序数");
        }
        "6" => {
            push_sym(&mut candidates, "⑥", "i6", "带圈序号");
            push_sym(&mut candidates, "⑹", "i6", "括号序号");
            push_sym(&mut candidates, "⒍", "i6", "带点序号");
            push_sym(&mut candidates, "Ⅵ", "i6", "罗马数字");
            push_sym(&mut candidates, "陆", "i6", "大写数字");
            push_sym(&mut candidates, "第六", "i6", "中文序数");
        }
        "7" => {
            push_sym(&mut candidates, "⑦", "i7", "带圈序号");
            push_sym(&mut candidates, "⑺", "i7", "括号序号");
            push_sym(&mut candidates, "⒎", "i7", "带点序号");
            push_sym(&mut candidates, "Ⅶ", "i7", "罗马数字");
            push_sym(&mut candidates, "柒", "i7", "大写数字");
            push_sym(&mut candidates, "第七", "i7", "中文序数");
        }
        "8" => {
            push_sym(&mut candidates, "⑧", "i8", "带圈序号");
            push_sym(&mut candidates, "⑻", "i8", "括号序号");
            push_sym(&mut candidates, "⒏", "i8", "带点序号");
            push_sym(&mut candidates, "Ⅷ", "i8", "罗马数字");
            push_sym(&mut candidates, "捌", "i8", "大写数字");
            push_sym(&mut candidates, "第八", "i8", "中文序数");
        }
        "9" => {
            push_sym(&mut candidates, "⑨", "i9", "带圈序号");
            push_sym(&mut candidates, "⑼", "i9", "括号序号");
            push_sym(&mut candidates, "⒐", "i9", "带点序号");
            push_sym(&mut candidates, "Ⅸ", "i9", "罗马数字");
            push_sym(&mut candidates, "玖", "i9", "大写数字");
            push_sym(&mut candidates, "第九", "i9", "中文序数");
        }
        "0" => {
            push_sym(&mut candidates, "⓪", "i0", "带圈数字");
            push_sym(&mut candidates, "⓿", "i0", "黑底数字");
            push_sym(&mut candidates, "〇", "i0", "中文零");
            push_sym(&mut candidates, "零", "i0", "大写数字");
        }
        "10" => {
            push_sym(&mut candidates, "⑩", "i10", "带圈序号");
            push_sym(&mut candidates, "⑽", "i10", "括号序号");
            push_sym(&mut candidates, "⒑", "i10", "带点序号");
            push_sym(&mut candidates, "Ⅹ", "i10", "罗马数字");
            push_sym(&mut candidates, "拾", "i10", "大写数字");
            push_sym(&mut candidates, "第十", "i10", "中文序数");
        }
        // 2. 数学运算与逻辑符号
        "+" | "jia" => {
            push_sym(&mut candidates, "±", "i+", "正负号");
            push_sym(&mut candidates, "∓", "i+", "负正号");
            push_sym(&mut candidates, "➕", "i+", "加号");
            push_sym(&mut candidates, "＋", "i+", "全角加号");
        }
        "-" | "jian" => {
            push_sym(&mut candidates, "－", "i-", "减号");
            push_sym(&mut candidates, "➖", "i-", "减号");
            push_sym(&mut candidates, "∓", "i-", "负正号");
            push_sym(&mut candidates, "—", "i-", "破折号");
        }
        "*" | "cheng" => {
            push_sym(&mut candidates, "×", "i*", "乘号");
            push_sym(&mut candidates, "✖", "i*", "粗乘号");
            push_sym(&mut candidates, "·", "i*", "间隔点");
            push_sym(&mut candidates, "★", "i*", "五角星");
            push_sym(&mut candidates, "☆", "i*", "空心星");
        }
        "/" | "chu" => {
            push_sym(&mut candidates, "÷", "i/", "除号");
            push_sym(&mut candidates, "➗", "i/", "除号");
            push_sym(&mut candidates, "／", "i/", "全角斜杠");
            push_sym(&mut candidates, "‰", "i/", "千分号");
            push_sym(&mut candidates, "‱", "i/", "万分号");
        }
        "=" | "deng" => {
            push_sym(&mut candidates, "≠", "i=", "不等号");
            push_sym(&mut candidates, "≈", "i=", "约等于");
            push_sym(&mut candidates, "≡", "i=", "恒等于");
            push_sym(&mut candidates, "≤", "i=", "小于等于");
            push_sym(&mut candidates, "≥", "i=", "大于等于");
            push_sym(&mut candidates, "≌", "i=", "全等");
        }
        "pi" => {
            push_sym(&mut candidates, "π", "ipi", "圆周率小写");
            push_sym(&mut candidates, "Π", "ipi", "圆周率大写");
        }
        "deg" | "du" => {
            push_sym(&mut candidates, "°", "ideg", "度数");
            push_sym(&mut candidates, "℃", "ideg", "摄氏度");
            push_sym(&mut candidates, "℉", "ideg", "华氏度");
        }
        "inf" => {
            push_sym(&mut candidates, "∞", "iinf", "无穷大");
            push_sym(&mut candidates, "∝", "iinf", "正比于");
        }
        "sum" => {
            push_sym(&mut candidates, "∑", "isum", "求和符号");
        }
        "sqrt" | "gen" => {
            push_sym(&mut candidates, "√", "isqrt", "平方根");
            push_sym(&mut candidates, "∛", "isqrt", "立方根");
            push_sym(&mut candidates, "∜", "isqrt", "四次方根");
        }
        // 3. 方向箭头与常用几何符号
        "jt" | "arrow" => {
            push_sym(&mut candidates, "→", "ijt", "向右箭头");
            push_sym(&mut candidates, "←", "ijt", "向左箭头");
            push_sym(&mut candidates, "↑", "ijt", "向上箭头");
            push_sym(&mut candidates, "↓", "ijt", "向下箭头");
            push_sym(&mut candidates, "⇄", "ijt", "左右交换");
            push_sym(&mut candidates, "↔", "ijt", "双向箭头");
            push_sym(&mut candidates, "⇒", "ijt", "右双箭头");
            push_sym(&mut candidates, "⇐", "ijt", "左双箭头");
        }
        "dui" | "gou" => {
            push_sym(&mut candidates, "✓", "idui", "对号");
            push_sym(&mut candidates, "✔", "idui", "粗对号");
            push_sym(&mut candidates, "√", "idui", "根号/对勾");
        }
        "cuo" | "cha" => {
            push_sym(&mut candidates, "✗", "icuo", "错号");
            push_sym(&mut candidates, "✘", "icuo", "粗错号");
            push_sym(&mut candidates, "×", "icuo", "叉号");
        }
        "xing" | "star" => {
            push_sym(&mut candidates, "★", "ixing", "实心星");
            push_sym(&mut candidates, "☆", "ixing", "空心星");
            push_sym(&mut candidates, "✪", "ixing", "圈星");
            push_sym(&mut candidates, "⭐", "ixing", "星星");
        }
        "xin" | "heart" => {
            push_sym(&mut candidates, "❤", "ixin", "实心红心");
            push_sym(&mut candidates, "♡", "ixin", "空心心形");
            push_sym(&mut candidates, "♥", "ixin", "黑心");
        }
        "fang" | "square" => {
            push_sym(&mut candidates, "■", "ifang", "实心方块");
            push_sym(&mut candidates, "□", "ifang", "空心方块");
            push_sym(&mut candidates, "◆", "ifang", "实心菱形");
            push_sym(&mut candidates, "◇", "ifang", "空心菱形");
            push_sym(&mut candidates, "▲", "ifang", "实心三角");
            push_sym(&mut candidates, "△", "ifang", "空心三角");
        }
        "yuan" | "circle" => {
            push_sym(&mut candidates, "●", "iyuan", "实心圆");
            push_sym(&mut candidates, "○", "iyuan", "空心圆");
            push_sym(&mut candidates, "◎", "iyuan", "双重圆");
            push_sym(&mut candidates, "☉", "iyuan", "圆心点");
        }
        // 4. 单位与货币
        "m" => {
            push_sym(&mut candidates, "㎡", "im", "平方米");
            push_sym(&mut candidates, "㎥", "im", "立方米");
            push_sym(&mut candidates, "㎜", "im", "毫米");
            push_sym(&mut candidates, "㎝", "im", "厘米");
            push_sym(&mut candidates, "㎞", "im", "千米");
        }
        "rmb" | "money" => {
            push_sym(&mut candidates, "¥", "irmb", "人民币/日元");
            push_sym(&mut candidates, "￥", "irmb", "全角人民币");
            push_sym(&mut candidates, "$", "irmb", "美元符号");
            push_sym(&mut candidates, "€", "irmb", "欧元符号");
            push_sym(&mut candidates, "£", "irmb", "英镑符号");
            push_sym(&mut candidates, "₩", "irmb", "韩元符号");
        }
        _ => {}
    }

    candidates
}

/// 辅助追加符号候选，保持分数从 10000 递减
fn push_sym(candidates: &mut Vec<AlgoCandidate>, text: &str, code: &str, comment: &str) {
    let score = (10_000u32).saturating_sub(candidates.len() as u32 * 500);
    candidates.push(AlgoCandidate::new(text, code, score).with_comment(comment));
}
