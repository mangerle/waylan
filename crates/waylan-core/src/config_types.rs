use serde::{Deserialize, Serialize};

/// 中英切换键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SwitchKey {
    #[default]
    Shift,
    Ctrl,
}

/// 主题模式选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

/// 候选翻页快捷键配置（支持多选同时生效）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PageKeys {
    /// 减号与等号 (- / =)
    pub minus_equal: bool,
    /// 左右中括号 ([ / ])
    pub bracket: bool,
    /// 逗号与句号 (, / .)
    pub comma_period: bool,
}

impl Default for PageKeys {
    fn default() -> Self {
        Self {
            minus_equal: true,
            bracket: false,
            comma_period: false,
        }
    }
}

/// 辅助反序列化枚举，同时支持新版结构体与旧版字符串枚举两种形态，保障版本平滑过渡
#[derive(Deserialize)]
#[serde(untagged)]
enum PageKeysHelper {
    Struct {
        #[serde(default)]
        minus_equal: bool,
        #[serde(default)]
        bracket: bool,
        #[serde(default)]
        comma_period: bool,
    },
    LegacyStr(String),
}

impl<'de> Deserialize<'de> for PageKeys {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match PageKeysHelper::deserialize(deserializer)? {
            PageKeysHelper::Struct {
                minus_equal,
                bracket,
                comma_period,
            } => Ok(Self {
                minus_equal,
                bracket,
                comma_period,
            }),
            PageKeysHelper::LegacyStr(s) => match s.as_str() {
                "MinusEqual" => Ok(Self {
                    minus_equal: true,
                    bracket: false,
                    comma_period: false,
                }),
                "Bracket" => Ok(Self {
                    minus_equal: false,
                    bracket: true,
                    comma_period: false,
                }),
                "CommaPeriod" => Ok(Self {
                    minus_equal: false,
                    bracket: false,
                    comma_period: true,
                }),
                "PageUpPageDown" => Ok(Self {
                    minus_equal: false,
                    bracket: false,
                    comma_period: false,
                }),
                _ => Ok(Self::default()),
            },
        }
    }
}

/// 候选框排列方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CandidateOrientation {
    /// 水平排列（默认）
    #[default]
    Horizontal,
    /// 垂直排列
    Vertical,
}

/// 候选词序号展示风格
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CandidateIndexStyle {
    /// 现代空白风：1 候选词（纯数字 + 半角空格，最干净利落，消除点号噪点）
    #[default]
    ModernSpace,
    /// 经典点号风：1. 候选词（传统输入法习惯）
    ClassicDot,
    /// 圆圈序号风：① 候选词（优雅紧凑，非常适合中文字形）
    Circled,
    /// 括号提示风：[1] 候选词
    SquareBracket,
    /// 圆括号提示风：(1) 候选词
    Parenthesis,
    /// 全极简无序号：仅在首选词加小圆点标记，后续词纯文本
    MinimalFirstDot,
    /// 纯文本完全无序号（适合纯空格选词的盲打用户）
    None,
}

/// 模糊音细分规则配置（对应模糊音设置页面 10 项开关）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FuzzyPinyinRules {
    /// c <=> ch
    pub c_ch: bool,
    /// z <=> zh
    pub z_zh: bool,
    /// s <=> sh
    pub s_sh: bool,
    /// an <=> ang
    pub an_ang: bool,
    /// en <=> eng
    pub en_eng: bool,
    /// in <=> ing
    pub in_ing: bool,
    /// l <=> n
    pub l_n: bool,
    /// f <=> h
    pub f_h: bool,
    /// l <=> r
    pub r_l: bool,
    /// g <=> k
    pub g_k: bool,
}

impl Default for FuzzyPinyinRules {
    fn default() -> Self {
        Self {
            c_ch: true,
            z_zh: true,
            s_sh: true,
            an_ang: true,
            en_eng: true,
            in_ing: true,
            l_n: true,
            f_h: true,
            r_l: false,
            g_k: false,
        }
    }
}

/// 拼音纠错细分规则配置（对应拼音模式页面拼音纠错开关）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TypoCorrectionRules {
    /// 是否启用拼音纠错主开关
    pub enable: bool,
    /// gn => ng
    pub gn_ng: bool,
    /// mg => ng
    pub mg_ng: bool,
    /// iou => iu
    pub iou_iu: bool,
    /// uei => ui
    pub uei_ui: bool,
    /// uen => un
    pub uen_un: bool,
    /// ue => ve
    pub ue_ve: bool,
    /// v => u, ve => ue, van => uan 等
    pub v_u: bool,
    /// on => ong
    pub on_ong: bool,
}

impl Default for TypoCorrectionRules {
    fn default() -> Self {
        Self {
            enable: true,
            gn_ng: true,
            mg_ng: true,
            iou_iu: true,
            uei_ui: true,
            uen_un: true,
            ue_ve: true,
            v_u: true,
            on_ong: true,
        }
    }
}

/// 简繁体即时切换快捷键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TraditionalShortcut {
    #[default]
    CtrlShiftF,
    CtrlAltF,
    CtrlShiftT,
    Disabled,
}

/// 字符全角/半角切换快捷键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FullWidthCharShortcut {
    #[default]
    ShiftSpace,
    CtrlShiftSpace,
    Disabled,
}

/// 标点全角/半角切换快捷键选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FullWidthPunctShortcut {
    #[default]
    CtrlPeriod,
    AltPeriod,
    Disabled,
}

/// 语音识别引擎类型选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VoiceEngineType {
    /// 本地离线端侧推理（Paraformer/Zipformer 等）
    #[default]
    Offline,
    /// 线上云端 API 识别（OpenAI/阿里/腾讯等）
    Cloud,
    /// 仿真测试（零外部依赖测试链路）
    Mock,
}

/// VAD 端点检测算法/模型类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VadModelType {
    /// Silero 神经网络 VAD 模型（基于 ONNX 权重文件）
    #[default]
    SileroOnnx,
    /// 轻量能量门限（零模型纯数学算法）
    Energy,
}

/// VAD 端点检测配置（支持自由更换模型与算法）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    /// VAD 类型
    pub vad_type: VadModelType,
    /// 自定义 VAD 模型文件路径（例如 "~/.local/share/waylan/models/vad/silero_vad.onnx"）
    pub model_path: Option<String>,
    /// 人声置信度门限（0.0 ~ 1.0）
    pub speech_threshold: f32,
    /// 静音超时判定时长（毫秒，默认 800ms）
    pub silence_timeout_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            vad_type: VadModelType::default(),
            model_path: None,
            speech_threshold: 0.5,
            silence_timeout_ms: 800,
        }
    }
}

/// 本地离线 ASR 识别模型架构类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OfflineModelType {
    /// 新一代 Zipformer 流式模型 (CTC / Transducer 极轻量架构)
    #[default]
    #[serde(
        alias = "Paraformer",
        alias = "paraformer",
        alias = "Whisper",
        alias = "whisper"
    )]
    Zipformer,
}

/// 本地离线 ASR 模型配置（支持自由更换模型文件与架构）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OfflineAsrConfig {
    /// 模型架构类型
    pub model_type: OfflineModelType,
    /// 模型文件所在根目录路径（例如 "~/.local/share/waylan/models/asr/paraformer-zh"）
    pub model_dir: Option<String>,
    /// 编码器模型文件名或相对路径
    pub encoder: String,
    /// 解码器模型文件名或相对路径
    pub decoder: String,
    /// 词表（Tokens）文件名或相对路径
    pub tokens: String,
}

impl Default for OfflineAsrConfig {
    fn default() -> Self {
        Self {
            model_type: OfflineModelType::default(),
            model_dir: None,
            encoder: "encoder.int8.onnx".to_string(),
            decoder: "decoder.int8.onnx".to_string(),
            tokens: "tokens.txt".to_string(),
        }
    }
}

/// 云端线上 ASR 服务提供商协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CloudProviderType {
    /// OpenAI 兼容音频转录接口（Whisper API）
    #[default]
    OpenAiCompatible,
    /// 阿里云流式语音识别
    Aliyun,
    /// 腾讯云流式语音识别
    Tencent,
    /// 自定义 WebSocket 流式服务端
    CustomWebSocket,
}

/// 线上云端语音识别配置（支持自由配置云端 API 与凭证）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudAsrConfig {
    /// 云端提供商协议
    pub provider: CloudProviderType,
    /// 线上服务接口地址（URL）
    pub endpoint: String,
    /// 鉴权 API Key / Token（敏感机密凭证）
    pub api_key: Option<String>,
    /// 指定的远端模型名称
    pub model: String,
}

impl Default for CloudAsrConfig {
    fn default() -> Self {
        Self {
            provider: CloudProviderType::default(),
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            api_key: None,
            model: "whisper-1".to_string(),
        }
    }
}

/// 智能降噪引擎类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NsEngineType {
    /// 内置极轻量 RNNoise (纯 Rust 静态集成，零外部依赖)
    #[default]
    Rnnoise,
    /// 深度滤波 DeepFilterNet (超强消除机械键盘敲击声)
    DeepFilterNet,
    /// 新一代 GTCRN 深度神经网络降噪 (523KB SOTA 极轻量)
    Gtcrn,
    /// 直通禁用降噪
    Bypass,
}

/// 线上云端语音识别服务配置实例（Profile）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudProfile {
    /// 唯一标识 ID（例如 "openai"、"groq"、"fastwhisper"）
    pub id: String,
    /// 界面友好展示备注名称（例如 "OpenAI 官方"、"Groq 极速转写"）
    pub name: String,
    /// 云端服务协议类型
    pub provider: CloudProviderType,
    /// 线上服务接口地址（URL）
    pub endpoint: String,
    /// 鉴权 API Key / Token（敏感机密凭据）
    pub api_key: Option<String>,
    /// 指定的远端模型名称（例如 "whisper-1"）
    pub model: String,
}

impl Default for CloudProfile {
    fn default() -> Self {
        Self {
            id: "default-openai".to_string(),
            name: "OpenAI 官方".to_string(),
            provider: CloudProviderType::default(),
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            api_key: None,
            model: "whisper-1".to_string(),
        }
    }
}

/// 音频前端预处理与降噪配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFrontEndConfig {
    /// 是否开启前端音频增强流水线总控（为 false 时全部旁路直通 Bypass）
    pub enabled: bool,
    /// 是否开启 80Hz 高通滤波（切除机械低频底噪与电磁交流声）
    pub high_pass_filter: bool,
    /// 是否开启人声智能降噪
    pub noise_suppression: bool,
    /// 智能降噪引擎模式（轻量 RNNoise 或深度 DeepFilterNet）
    pub ns_engine: NsEngineType,
    /// 是否开启数字自动增益控制 (AGC)
    pub auto_gain_control: bool,
    /// 自定义降噪模型文件路径（可选，支持从 models/ns/ 导入）
    pub custom_ns_model_path: Option<String>,
}

impl Default for AudioFrontEndConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            high_pass_filter: true,
            noise_suppression: true,
            ns_engine: NsEngineType::default(),
            auto_gain_control: true,
            custom_ns_model_path: None,
        }
    }
}

/// 语音输入配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// 是否开启语音输入功能
    pub enabled: bool,
    /// 连续听写开关快捷键（按一下开启，VAD分句上屏，按回车或再次按快捷键结束）
    pub toggle_shortcut: String,
    /// 长按对讲机快捷键（按住说话，松手即上屏）
    pub push_to_talk_shortcut: String,
    /// 识别引擎工作模式（本地离线 / 线上云端 / 仿真测试）
    pub engine_type: VoiceEngineType,
    /// 当前统一选中的 ASR 识别目标（为 None 或以 "local:" 开头时走本地；以 "cloud:<id>" 开头时走对应云端服务）
    pub selected_asr: Option<String>,
    /// 是否自动在识别文本末尾补全句号
    pub auto_punctuation: bool,
    /// 1. VAD 端点检测配置（支持自由更换模型与算法）
    pub vad: VadConfig,
    /// 2. 本地离线识别配置（支持自由更换模型与路径）
    pub offline: OfflineAsrConfig,
    /// 3. 线上云端识别配置（保留旧版本单一兼容配置）
    pub cloud: CloudAsrConfig,
    /// 3.1 线上云端多服务配置列表（支持配置多个独立的 Whisper 兼容服务）
    pub cloud_profiles: Vec<CloudProfile>,
    /// 4. 音频前端预处理与降噪配置 (HPF + NS + AGC)
    pub afe: AudioFrontEndConfig,
    /// 5. 文本规整与语气词过滤配置 (ITN)
    pub itn: VoiceItnConfig,
    /// 6. 交互展现与流式上屏配置
    pub interaction: VoiceInteractionConfig,
    /// 麦克风音频输入设备名称（为 None 时自动使用系统默认设备）
    pub device_name: Option<String>,
    /// 自定义标点符号预测模型文件路径（为 None 时自动使用系统内置或默认模型）
    pub punct_model_path: Option<String>,
}

/// 阶段五：文本规整与语气词过滤 (ITN) 配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceItnConfig {
    /// 是否开启大写数字自动转阿拉伯数字（如“三点五”转“3.5”）
    pub convert_numbers: bool,
    /// 是否开启口语停顿语气词与结巴自动过滤（过滤“呃、啊、这个、那个”等口癖）
    pub filter_filler_words: bool,
    /// 是否开启中英文自动盘古空格
    pub pangu_spacing: bool,
}

impl Default for VoiceItnConfig {
    fn default() -> Self {
        Self {
            convert_numbers: true,
            filter_filler_words: true,
            pangu_spacing: true,
        }
    }
}

/// 阶段六：交互展现与流式上屏配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceInteractionConfig {
    /// 是否在光标处实时文字预览（边说边出字；关闭后为松手/停顿后整句上屏）
    pub real_time_preview: bool,
    /// 是否显示录音浮动波形条
    pub show_floating_bar: bool,
}

impl Default for VoiceInteractionConfig {
    fn default() -> Self {
        Self {
            real_time_preview: true,
            show_floating_bar: true,
        }
    }
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            toggle_shortcut: "Ctrl + Alt + V".to_string(),
            push_to_talk_shortcut: "F8".to_string(),
            engine_type: VoiceEngineType::default(),
            selected_asr: None,
            auto_punctuation: true,
            vad: VadConfig::default(),
            offline: OfflineAsrConfig::default(),
            cloud: CloudAsrConfig::default(),
            cloud_profiles: vec![CloudProfile::default()],
            afe: AudioFrontEndConfig::default(),
            itn: VoiceItnConfig::default(),
            interaction: VoiceInteractionConfig::default(),
            device_name: None,
            punct_model_path: None,
        }
    }
}

/// 归一化解析后的快捷键数据
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParsedShortcut {
    /// 是否需要按住 Ctrl
    pub ctrl: bool,
    /// 是否需要按住 Alt
    pub alt: bool,
    /// 是否需要按住 Shift
    pub shift: bool,
    /// 是否需要按住 Super / Win
    pub super_key: bool,
    /// 目标主键的 Linux evdev 扫描码
    pub evdev_key: u32,
}

impl ParsedShortcut {
    /// 从用户输入的快捷键描述文本解析（如 "Ctrl+Alt+V", "Alt+Space", "F8", "Super+V" 等）
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("disabled") || trimmed == "已禁用"
        {
            return None;
        }

        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut super_key = false;
        let mut evdev_key = None;

        for part in trimmed
            .split('+')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
        {
            let lower = part.to_ascii_lowercase();
            match lower.as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "mod1" => alt = true,
                "shift" => shift = true,
                "super" | "win" | "meta" => super_key = true,
                _ => {
                    if let Some(code) = parse_key_name(&lower) {
                        evdev_key = Some(code);
                    }
                }
            }
        }

        evdev_key.map(|k| Self {
            ctrl,
            alt,
            shift,
            super_key,
            evdev_key: k,
        })
    }

    /// 校验当前输入事件是否精确匹配该快捷键
    pub fn matches(
        &self,
        evdev_key: u32,
        shift_active: bool,
        ctrl_active: bool,
        alt_active: bool,
        super_active: bool,
    ) -> bool {
        self.evdev_key == evdev_key
            && self.ctrl == ctrl_active
            && self.alt == alt_active
            && self.shift == shift_active
            && self.super_key == super_active
    }
}

/// 将常见按键小写名称映射为标准 Linux evdev 扫描码
fn parse_key_name(name: &str) -> Option<u32> {
    let code = match name {
        "space" | "空格" => 57,
        "enter" | "return" | "回车" => 28,
        "tab" => 15,
        "esc" | "escape" => 1,
        "backspace" => 14,
        "f1" => 59,
        "f2" => 60,
        "f3" => 61,
        "f4" => 62,
        "f5" => 63,
        "f6" => 64,
        "f7" => 65,
        "f8" => 66,
        "f9" => 67,
        "f10" => 68,
        "f11" => 87,
        "f12" => 88,
        "a" => 30,
        "b" => 48,
        "c" => 46,
        "d" => 32,
        "e" => 18,
        "f" => 33,
        "g" => 34,
        "h" => 35,
        "i" => 23,
        "j" => 36,
        "k" => 37,
        "l" => 38,
        "m" => 50,
        "n" => 49,
        "o" => 24,
        "p" => 25,
        "q" => 16,
        "r" => 19,
        "s" => 31,
        "t" => 20,
        "u" => 22,
        "v" => 47,
        "w" => 17,
        "x" => 45,
        "y" => 21,
        "z" => 44,
        "1" => 2,
        "2" => 3,
        "3" => 4,
        "4" => 5,
        "5" => 6,
        "6" => 7,
        "7" => 8,
        "8" => 9,
        "9" => 10,
        "0" => 11,
        "grave" | "`" => 41,
        "minus" | "-" => 12,
        "equal" | "=" => 13,
        "comma" | "," => 51,
        "period" | "dot" | "." => 52,
        "slash" | "/" => 53,
        "semicolon" | ";" => 39,
        _ => return None,
    };
    Some(code)
}

impl VoiceConfig {
    /// 获取当前生效的连续听写开关快捷键 (Toggle)
    pub fn get_effective_toggle_shortcut(&self) -> Option<ParsedShortcut> {
        if !self.enabled {
            return None;
        }
        ParsedShortcut::parse(&self.toggle_shortcut)
    }

    /// 获取当前生效的长按对讲机快捷键 (Push-to-Talk)
    pub fn get_effective_ptt_shortcut(&self) -> Option<ParsedShortcut> {
        if !self.enabled {
            return None;
        }
        ParsedShortcut::parse(&self.push_to_talk_shortcut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsed_shortcut_parsing() {
        let p1 = ParsedShortcut::parse("Ctrl + Alt + V").expect("必须成功解析");
        assert!(p1.ctrl);
        assert!(p1.alt);
        assert!(!p1.shift);
        assert!(!p1.super_key);
        assert_eq!(p1.evdev_key, 47);

        let p2 = ParsedShortcut::parse("F8").expect("单键必须成功解析");
        assert!(!p2.ctrl);
        assert!(!p2.alt);
        assert_eq!(p2.evdev_key, 66);

        let p3 = ParsedShortcut::parse("Super + Shift + V").expect("Super键解析");
        assert!(p3.super_key);
        assert!(p3.shift);
        assert_eq!(p3.evdev_key, 47);

        assert!(ParsedShortcut::parse("已禁用").is_none());
        assert!(ParsedShortcut::parse("   ").is_none());
    }

    #[test]
    fn test_voice_config_effective_dual_shortcuts() {
        let mut cfg = VoiceConfig::default();
        // 默认状态下语音为关闭状态，所有快捷键均不应生效
        assert!(cfg.get_effective_toggle_shortcut().is_none());
        assert!(cfg.get_effective_ptt_shortcut().is_none());

        // 显式开启语音输入功能
        cfg.enabled = true;
        let toggle = cfg
            .get_effective_toggle_shortcut()
            .expect("开启后默认开关按键必须生效");
        assert!(toggle.matches(47, false, true, true, false));

        let ptt = cfg
            .get_effective_ptt_shortcut()
            .expect("开启后默认对讲机按键必须生效");
        assert!(ptt.matches(66, false, false, false, false));

        cfg.toggle_shortcut = "已禁用".to_string();
        assert!(cfg.get_effective_toggle_shortcut().is_none());
        assert!(cfg.get_effective_ptt_shortcut().is_some());

        cfg.enabled = false;
        assert!(cfg.get_effective_ptt_shortcut().is_none());
    }

    #[test]
    fn test_audio_front_end_config_defaults_and_serde() {
        let default_afe = AudioFrontEndConfig::default();
        assert!(default_afe.high_pass_filter);
        assert!(default_afe.noise_suppression);
        assert_eq!(default_afe.ns_engine, NsEngineType::Rnnoise);
        assert!(default_afe.auto_gain_control);
        assert!(default_afe.custom_ns_model_path.is_none());

        // 验证空 JSON 反序列化可自动使用全量默认值
        let json_empty = "{}";
        let parsed: AudioFrontEndConfig =
            serde_json::from_str(json_empty).expect("空 JSON 必须成功使用默认值反序列化");
        assert_eq!(parsed, default_afe);

        // 验证序列化与恢复
        let serialized = serde_json::to_string(&default_afe).expect("必须成功序列化");
        let restored: AudioFrontEndConfig =
            serde_json::from_str(&serialized).expect("必须成功从序列化还原");
        assert_eq!(restored, default_afe);
    }
}
