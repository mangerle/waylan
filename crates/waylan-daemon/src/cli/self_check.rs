//! 核心算法与词库完整性自检模块
//!
//! # 设计原理
//! - **实现初衷**：在守护进程启动前或通过 `--check` 参数提供确定性的算法与词库健康检查。
//! - **核心优势**：模拟典型多歧义切分按键（如 "xian" 同时分流为 "xian" 和 "xi'an"），
//!   验证词典倒排索引、Prism 拼写图与候选词流水线的连通性，及早暴露缺失词库文件。

use crate::error::DaemonError;
use log::info;
use waylan_algo::AlgoEngine;

/// 执行算法与词典自检测试
///
/// # Errors
/// 当引擎内部状态异常或未能召回任何候选词时返回校验失败错误。
pub fn run_self_check(engine: &mut AlgoEngine) -> Result<(), DaemonError> {
    info!("=== 微澜核心算法引擎自检 ===");
    for ch in "xian".chars() {
        engine.push_char(ch);
    }
    let cands = engine.candidates();
    info!("输入: 'xian' => 召回候选数: {}", cands.len());
    for (i, c) in cands.iter().enumerate().take(5) {
        info!(
            "  [{}] {} (拼音: {}, 得分: {})",
            i + 1,
            c.text,
            c.pinyin,
            c.score
        );
    }
    let is_empty = cands.is_empty();
    engine.reset();
    if is_empty {
        return Err(DaemonError::Validation(
            "自检测试失败：键入 'xian' 未能召回任何候选词，请检查词库完整性！".into(),
        ));
    }
    info!("自检测试通过！");
    Ok(())
}
