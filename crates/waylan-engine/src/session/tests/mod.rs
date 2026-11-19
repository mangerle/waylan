//! 核心会话按键事件分发流水线单元测试

use super::*;
use waylan_algo::table::LexiconTable;
use waylan_algo::types::AlgoConfig;

pub(super) fn make_test_session() -> ImeCoreSession {
    let engine = AlgoEngine::new(LexiconTable::with_sample_data(), AlgoConfig::default());
    ImeCoreSession::new(engine)
}

mod basic;
mod navigation;
mod shortcuts;
