//! IBus 会话按键核心状态与决策结果结构定义
//!
//! # 设计原理
//! - **实现初衷**：维护输入法引擎的核心上下文（拼音状态、修饰键跟踪、中英模式、全半角配置与语音控制器句柄）。
//! - **核心优势**：纯内存数据结构，加锁保护仅限于单次按键决策的微秒级临界区内。
//! - **代价与局限**：受 Mutex 保护，不可跨 await 持有。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use waylan_algo::preprocessor::{PunctuationProcessor, ShiftToggleStateMachine};
use waylan_algo::AlgoEngine;
use waylan_core::input::sync_mode_from_shared_flag;
use waylan_core::input::{ConsumedKeyTracker, PageKeyConfig};
use waylan_core::{
    Candidate, CandidateIndexStyle, CandidateOrientation, FullWidthCharShortcut,
    FullWidthPunctShortcut, InputMode, TraditionalShortcut, VoiceConfig,
};
use waylan_voice::VoiceInteractionManager;

/// 单次 IBus 按键或交互事件的决策输出
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IbusKeyOutcome {
    /// 是否由输入法拦截消费该按键
    pub consumed: bool,
    /// 需要立即提交上屏的文本（若有）
    pub commit_text: Option<String>,
    /// 是否需要刷新预编辑文本与候选词表信号
    pub update_ui: bool,
    /// 若发生中英文状态切换，记录切换后的新模式
    pub mode_changed: Option<InputMode>,
    /// 成对标点补全后需模拟方向键移动光标的偏移量（负数表示向左）
    pub cursor_offset: i32,
}

/// 微澜 IBus 会话内部核心状态机（受互斥锁保护，在 D-Bus 方法调用中同步推进）
pub struct IbusSessionCore {
    pub engine: AlgoEngine,
    pub shift_sm: ShiftToggleStateMachine,
    pub punct_processor: PunctuationProcessor,
    pub page_keys: PageKeyConfig,
    pub mode: InputMode,
    pub shared_is_chinese: Option<Arc<AtomicBool>>,
    pub shared_is_caps_locked: Option<Arc<AtomicBool>>,
    pub shared_traditional_chinese: Option<Arc<AtomicBool>>,
    pub shared_full_width_punct: Option<Arc<AtomicBool>>,
    pub shared_full_width_char: Option<Arc<AtomicBool>>,
    pub mode_notify_tx: Option<SyncSender<()>>,
    pub is_caps_locked: bool,
    pub mods_depressed: u32,
    pub phys_mods: u8,
    pub consumed_keys: ConsumedKeyTracker,
    pub is_password_context: bool,
    pub is_screen_locked: bool,
    pub surrounding_text: Option<(String, u32, u32)>,
    pub candidate_index_style: CandidateIndexStyle,
    pub candidate_orientation: CandidateOrientation,
    pub traditional_shortcut: TraditionalShortcut,
    pub full_width_char_shortcut: FullWidthCharShortcut,
    pub full_width_punct_shortcut: FullWidthPunctShortcut,
    pub voice_manager: VoiceInteractionManager,
    /// 当前拥有焦点的活跃 IBus 引擎对象路径
    pub active_engine_path: Option<String>,
}

impl IbusSessionCore {
    /// 使用预加载的算法引擎构造 IBus 会话状态核心
    pub fn new(engine: AlgoEngine) -> Self {
        Self {
            engine,
            shift_sm: ShiftToggleStateMachine::new(),
            punct_processor: PunctuationProcessor::new(),
            page_keys: PageKeyConfig::default(),
            mode: InputMode::Chinese,
            shared_is_chinese: None,
            shared_is_caps_locked: None,
            shared_traditional_chinese: None,
            shared_full_width_punct: None,
            shared_full_width_char: None,
            mode_notify_tx: None,
            is_caps_locked: false,
            mods_depressed: 0,
            phys_mods: 0,
            consumed_keys: ConsumedKeyTracker::new(),
            is_password_context: false,
            is_screen_locked: false,
            surrounding_text: None,
            candidate_index_style: CandidateIndexStyle::default(),
            candidate_orientation: CandidateOrientation::default(),
            traditional_shortcut: TraditionalShortcut::default(),
            full_width_char_shortcut: FullWidthCharShortcut::default(),
            full_width_punct_shortcut: FullWidthPunctShortcut::default(),
            voice_manager: VoiceInteractionManager::new(VoiceConfig::default()),
            active_engine_path: None,
        }
    }

    /// 同步来自系统托盘的共享中英文状态标志
    pub fn sync_shared_mode(&mut self) -> Option<InputMode> {
        sync_mode_from_shared_flag(self.shared_is_chinese.as_deref(), &mut self.mode)
    }

    /// 同步来自托盘与全局状态的简繁与标点/字符全半角配置
    pub fn sync_shared_settings(&mut self) {
        if let Some(ref trad) = self.shared_traditional_chinese {
            let val = trad.load(Ordering::Acquire);
            if self.engine.config.traditional_chinese != val {
                self.engine.config.traditional_chinese = val;
            }
        }
        if let Some(ref punct) = self.shared_full_width_punct {
            let val = punct.load(Ordering::Acquire);
            if self.punct_processor.full_width_punct != val {
                self.punct_processor.full_width_punct = val;
            }
        }
    }

    /// 判定当前是否开启了全角字符/字母模式
    pub fn is_full_width_char(&self) -> bool {
        self.shared_full_width_char
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Acquire))
    }

    /// 重置当前预编辑与修饰键跟踪状态（用于失焦 `FocusOut` 或 `Reset`）
    pub fn reset_context(&mut self) {
        if self.voice_manager.is_recording() {
            self.voice_manager.cancel();
        }
        self.voice_manager.clear_state();
        self.engine.reset();
        self.shift_sm.reset();
        self.mods_depressed = 0;
        self.phys_mods = 0;
        self.consumed_keys.clear();
        self.is_password_context = false;
        self.surrounding_text = None;
    }

    /// 获取当前页的候选词列表（若处于二级展开状态，则返回对应展开子项列表）
    pub fn current_candidates(&self) -> Vec<Candidate> {
        if self.engine.is_expanded {
            if let Some(cand) = self.engine.candidates().get(self.engine.selected_index) {
                if cand.is_expandable && !cand.sub_candidates.is_empty() {
                    let mut out = Vec::with_capacity(cand.sub_candidates.len());
                    for sub in &cand.sub_candidates {
                        out.push(Candidate {
                            text: sub.clone(),
                            pinyin: cand.pinyin.clone(),
                            score: cand.score,
                            is_expandable: false,
                            sub_candidates: Vec::new(),
                        });
                    }
                    return out;
                }
            }
        }
        let cands = self.engine.candidates();
        let mut out = Vec::with_capacity(cands.len());
        for c in cands {
            out.push(Candidate {
                text: c.text.clone(),
                pinyin: c.pinyin.clone(),
                score: c.score,
                is_expandable: c.is_expandable,
                sub_candidates: c.sub_candidates.clone(),
            });
        }
        out
    }

    /// 同步 CapsLock 大写锁定状态至共享标志并唤醒托盘刷新
    pub(crate) fn update_caps_lock_state(&mut self, is_locked: bool) {
        if self.is_caps_locked != is_locked {
            self.is_caps_locked = is_locked;
            if let Some(ref flag) = self.shared_is_caps_locked {
                flag.store(is_locked, Ordering::Release);
            }
            if let Some(ref tx) = self.mode_notify_tx {
                let _ = tx.try_send(());
            }
        }
    }
}
