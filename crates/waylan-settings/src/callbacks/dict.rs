//! 词库统计信息装配与外部词库导入回调
//!
//! # 设计原理
//! - **实现初衷**：将基础系统词库、用户自造词模型、辅助英文与特殊符号库的统计及文件选取异步导入解耦至独立模块。
//! - **核心优势**：职责内聚，包含原子防重入标志保护与文件对话框异步派发，杜绝 UI 线程阻塞卡顿。
//! - **代价与局限**：回调函数需捕获 Slint 窗口 Weak 引用与各词库原子锁。

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use waylan_engine::{AuxiliaryDict, CustomPhraseManager, LexiconSummaryService};

use super::desktop::pick_file_dialog_async;
use super::tasks::{
    spawn_english_import_task, spawn_pinyin_import_task, spawn_symbols_import_task,
};
use crate::{PhraseItem, SettingsWindow};

/// 词库与短语初始化统计模型集合
pub struct LexiconStatsBundle {
    /// 辅助与混输词库（英文与表情符号）
    pub aux_dict: AuxiliaryDict,
    /// 界面自定义短语数据模型
    pub phrases_model: Rc<VecModel<PhraseItem>>,
    /// 用户自定义短语文件绝对路径
    pub phrase_path: PathBuf,
}

/// 统计与装载词库信息到窗口
pub fn populate_lexicon_statistics(window: &SettingsWindow) -> LexiconStatsBundle {
    let phrase_path = CustomPhraseManager::default_path();
    let phrase_manager = CustomPhraseManager::load_or_init(&phrase_path);
    let phrases_data: Vec<PhraseItem> = phrase_manager
        .all_entries()
        .into_iter()
        .map(|(code, text)| PhraseItem {
            code: SharedString::from(code),
            text: SharedString::from(text),
        })
        .collect();
    let phrases_model = Rc::new(VecModel::from(phrases_data));
    window.set_phrase_list(ModelRc::from(phrases_model.clone()));

    // 辅助词库装载与展示
    let aux_dict = LexiconSummaryService::load_auxiliary_dict();
    window.set_english_dict_info(SharedString::from(format!(
        "已就绪常用英文单词: {} 词",
        aux_dict.english_words.len()
    )));
    window.set_symbols_dict_info(SharedString::from(format!(
        "已收录语义符号与表情: {} 项",
        aux_dict.symbols.len()
    )));

    // 自造词模型统计
    let user_model_count = LexiconSummaryService::query_user_adaptive_entry_count();
    window.set_user_model_info(SharedString::from(format!(
        "自适应个性化历史词模型: {} 条记忆",
        user_model_count
    )));

    // 系统预编译主词库状态统计
    let base_dict_info = LexiconSummaryService::query_system_dict_info();
    window.set_base_dict_info(SharedString::from(base_dict_info));

    LexiconStatsBundle {
        aux_dict,
        phrases_model,
        phrase_path,
    }
}

/// 注册拼音、英文与符号等外部词库文件选取与异步导入回调
pub(crate) fn register_dict_import_callbacks(window: &SettingsWindow, aux_dict: AuxiliaryDict) {
    // 拼音词库导入回调
    let is_pinyin_importing = Arc::new(AtomicBool::new(false));
    let pinyin_running = is_pinyin_importing;
    let window_weak = window.as_weak();
    window.on_pick_and_import_pinyin(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_pick_and_import_pinyin(&win, &pinyin_running);
        }
    });

    // 英文与符号辅助词库导入回调（共享同一个互斥锁与防重入标志）
    let is_aux_importing = Arc::new(AtomicBool::new(false));
    let aux_dict_lock = Arc::new(Mutex::new(aux_dict));

    let window_weak = window.as_weak();
    let aux_for_en = aux_dict_lock.clone();
    let aux_running_for_en = is_aux_importing.clone();
    window.on_pick_and_import_english(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_pick_and_import_english(&win, &aux_for_en, &aux_running_for_en);
        }
    });

    let window_weak = window.as_weak();
    let aux_for_sym = aux_dict_lock;
    let aux_running_for_sym = is_aux_importing;
    window.on_pick_and_import_symbols(move || {
        if let Some(win) = window_weak.upgrade() {
            handle_pick_and_import_symbols(&win, &aux_for_sym, &aux_running_for_sym);
        }
    });
}

/// 在 UI 线程回填拼音词库文件选择结果并派发后台导入任务
fn apply_pinyin_selection(
    win_weak: slint::Weak<SettingsWindow>,
    running_flag: Arc<AtomicBool>,
    selected: Option<PathBuf>,
) {
    let Some(win) = win_weak.upgrade() else {
        running_flag.store(false, Ordering::Release);
        return;
    };
    let Some(path) = selected else {
        win.set_pinyin_import_status(SharedString::from("已取消文件选择"));
        running_flag.store(false, Ordering::Release);
        return;
    };
    let path_str = path.to_string_lossy().to_string();
    win.set_pinyin_import_path(SharedString::from(&path_str));
    win.set_pinyin_import_status(SharedString::from("正在解析并合入词库，请稍候..."));
    spawn_pinyin_import_task(win.as_weak(), path_str, running_flag);
}

/// 处理拼音词库文件选取与异步导入派发
fn handle_pick_and_import_pinyin(win: &SettingsWindow, running_flag: &Arc<AtomicBool>) {
    if running_flag
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        win.set_pinyin_import_status(SharedString::from(
            "已有拼音词库导入任务正在执行，请稍候...",
        ));
        return;
    }
    win.set_pinyin_import_status(SharedString::from("正在等待选择文件..."));
    let win_weak = win.as_weak();
    let running_flag = running_flag.clone();

    pick_file_dialog_async(
        "请选择要导入的拼音词库文件",
        "词库文件 (*.txt, *.dict.yaml, *.yaml, *.scel)",
        "*.txt *.dict.yaml *.yaml *.yml *.scel",
        move |selected| {
            let _ = slint::invoke_from_event_loop(move || {
                apply_pinyin_selection(win_weak, running_flag, selected);
            });
        },
    );
}

/// 辅助词库后台导入任务派发函数签名
type AuxDictSpawnTask =
    fn(slint::Weak<SettingsWindow>, String, Arc<Mutex<AuxiliaryDict>>, Arc<AtomicBool>);

/// 辅助词库（英文/符号）导入链路描述
///
/// # 设计原理
/// - **实现初衷**：英文词库与特殊符号库的「防重入 -> 弹选择器 -> 回投 UI 回填 -> 派后台任务」
///   四步流程逐字一致，仅界面属性写入器、对话框文案与后台任务函数不同。
/// - **核心优势**：以声明式链路描述表消除两套回调闭包的重复，降级路径（窗口销毁/用户取消）单点实现。
/// - **代价与局限**：新增辅助词库种类时需补充一张链路描述常量表。
#[derive(Clone, Copy)]
struct AuxDictImportFlow {
    /// 导入状态属性写入器
    status_setter: fn(&SettingsWindow, SharedString),
    /// 已选文件路径属性写入器
    path_setter: fn(&SettingsWindow, SharedString),
    /// 文件选择对话框标题
    dialog_title: &'static str,
    /// 文件选择对话框类型说明
    dialog_filter_label: &'static str,
    /// 文件选择对话框匹配模式
    dialog_patterns: &'static str,
    /// 解析阶段提示文案
    parsing_hint: &'static str,
    /// 后台导入任务派发函数
    spawn_task: AuxDictSpawnTask,
}

/// 英文辅助词库导入链路
const ENGLISH_IMPORT_FLOW: AuxDictImportFlow = AuxDictImportFlow {
    status_setter: SettingsWindow::set_english_import_status,
    path_setter: SettingsWindow::set_english_import_path,
    dialog_title: "请选择要导入的英文词库文件",
    dialog_filter_label: "英文词汇与词频文件 (*.txt, *.dict.yaml, *.yaml)",
    dialog_patterns: "*.txt *.dict.yaml *.yaml *.yml",
    parsing_hint: "正在解析英文词库，请稍候...",
    spawn_task: spawn_english_import_task,
};

/// 特殊符号辅助词库导入链路
const SYMBOLS_IMPORT_FLOW: AuxDictImportFlow = AuxDictImportFlow {
    status_setter: SettingsWindow::set_symbols_import_status,
    path_setter: SettingsWindow::set_symbols_import_path,
    dialog_title: "请选择要导入的特殊符号库文件",
    dialog_filter_label: "符号定义文件 (*.txt, *.dict.yaml, *.yaml)",
    dialog_patterns: "*.txt *.dict.yaml *.yaml *.yml",
    parsing_hint: "正在解析特殊符号库，请稍候...",
    spawn_task: spawn_symbols_import_task,
};

/// 辅助词库导入派发上下文
///
/// # 设计原理
/// - **实现初衷**：文件对话框异步回调仅额外携带"用户选中的路径"这一项动态数据，
///   链路描述、窗口句柄、词库锁与防重入标志在派发前即已确定。
/// - **核心优势**：把稳定依赖收敛为按值移动的上下文，回调闭包退化为一次具名函数转发。
/// - **代价与局限**：上下文一次性消费，派发后不可复用（本流程亦无复用需求）。
#[derive(Clone)]
struct AuxDictPickContext {
    /// 所属导入链路描述
    flow: AuxDictImportFlow,
    /// 窗口弱引用
    win_weak: slint::Weak<SettingsWindow>,
    /// 辅助词库共享互斥锁
    aux_dict: Arc<Mutex<AuxiliaryDict>>,
    /// 防重入标志
    running_flag: Arc<AtomicBool>,
}

/// 将辅助词库文件选择结果回投 Slint 主线程处理
fn queue_aux_dict_selection(ctx: AuxDictPickContext, selected: Option<PathBuf>) {
    let _ = slint::invoke_from_event_loop(move || apply_aux_dict_selection(ctx, selected));
}

/// 在 UI 线程回填辅助词库选择结果并派发后台导入任务
///
/// 窗口销毁与用户取消两条降级路径均在此释放防重入标志，杜绝标志悬挂导致后续导入永久被拒。
fn apply_aux_dict_selection(ctx: AuxDictPickContext, selected: Option<PathBuf>) {
    let AuxDictPickContext {
        flow,
        win_weak,
        aux_dict,
        running_flag,
    } = ctx;
    let Some(win) = win_weak.upgrade() else {
        running_flag.store(false, Ordering::Release);
        return;
    };
    let Some(path) = selected else {
        (flow.status_setter)(&win, SharedString::from("已取消文件选择"));
        running_flag.store(false, Ordering::Release);
        return;
    };

    let path_str = path.to_string_lossy().to_string();
    (flow.path_setter)(&win, SharedString::from(&path_str));
    (flow.status_setter)(&win, SharedString::from(flow.parsing_hint));
    (flow.spawn_task)(win.as_weak(), path_str, aux_dict, running_flag);
}

/// 处理辅助词库文件选取与异步导入派发
fn handle_pick_and_import_aux_dict(
    win: &SettingsWindow,
    aux_dict: &Arc<Mutex<AuxiliaryDict>>,
    running_flag: &Arc<AtomicBool>,
    flow: AuxDictImportFlow,
) {
    if running_flag
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        (flow.status_setter)(
            win,
            SharedString::from("已有辅助词库导入任务正在执行，请稍候..."),
        );
        return;
    }
    (flow.status_setter)(win, SharedString::from("正在等待选择文件..."));

    let ctx = AuxDictPickContext {
        flow,
        win_weak: win.as_weak(),
        aux_dict: aux_dict.clone(),
        running_flag: running_flag.clone(),
    };

    pick_file_dialog_async(
        flow.dialog_title,
        flow.dialog_filter_label,
        flow.dialog_patterns,
        move |selected| queue_aux_dict_selection(ctx, selected),
    );
}

/// 处理英文辅助词库文件选取与异步导入派发
fn handle_pick_and_import_english(
    win: &SettingsWindow,
    aux_dict: &Arc<Mutex<AuxiliaryDict>>,
    running_flag: &Arc<AtomicBool>,
) {
    handle_pick_and_import_aux_dict(win, aux_dict, running_flag, ENGLISH_IMPORT_FLOW);
}

/// 处理特殊符号库文件选取与异步导入派发
fn handle_pick_and_import_symbols(
    win: &SettingsWindow,
    aux_dict: &Arc<Mutex<AuxiliaryDict>>,
    running_flag: &Arc<AtomicBool>,
) {
    handle_pick_and_import_aux_dict(win, aux_dict, running_flag, SYMBOLS_IMPORT_FLOW);
}
