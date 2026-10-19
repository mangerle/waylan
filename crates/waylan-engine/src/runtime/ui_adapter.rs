//! 前端呈现适配与候选框视图构建模块
//!
//! # 设计原理
//! - **实现初衷**：将对客户端界面契约 [`InputMethodFrontend`] 的刷新、重置与视图状态构建从主按键循环中解耦。
//! - **核心优势**：职责单一，统一根据核心会话与语音管理器的活跃态派生不可变的 [`CandidateBoxState`]。
//! - **代价与局限**：视图状态构建需聚合算法引擎的候选词、网格与语音胶囊条数据。

use waylan_core::CandidateBoxState;

use super::candidate_builder::{build_common_candidate_box_state, CandidateBoxBuildOptions};
use super::ImeRuntime;
use crate::frontend::InputMethodFrontend;

impl ImeRuntime {
    /// 重置会话与焦点上下文（在失焦 FocusOut 或客户端重置时调用）
    pub fn reset_context<F: InputMethodFrontend>(&mut self, frontend: &mut F) {
        if let Some(ref mut vm) = self.voice_manager {
            if vm.is_recording() {
                vm.cancel();
            }
            vm.clear_state();
        }
        self.core.reset_input_state();
        frontend.hide_candidates();
        frontend.update_preedit("", 0);
    }

    /// 统一刷新前端 UI（包括客户端预编辑与候选框卡片）
    pub fn refresh_frontend_ui<F: InputMethodFrontend>(
        &mut self,
        frontend: &mut F,
        inline_preedit: bool,
    ) {
        let is_voice = self.is_voice_active();
        let is_composing = self.is_composing();

        if !is_voice && !is_composing {
            frontend.hide_candidates();
            frontend.update_preedit("", 0);
            return;
        }

        // 1. 同步预编辑字符串给客户端
        let preedit_text = if is_composing {
            self.core.engine.formatted_preedit()
        } else {
            String::new()
        };
        let cursor_pos = if is_composing {
            self.core.engine.cursor()
        } else {
            0
        };
        frontend.update_preedit(&preedit_text, cursor_pos);

        // 2. 构建候选框状态并提交给前端呈现
        let box_state = self.build_candidate_box_state(true, inline_preedit);
        frontend.update_candidates(&box_state);
    }

    /// 构建不可变候选框渲染快照
    pub fn build_candidate_box_state(
        &self,
        is_active: bool,
        inline_preedit: bool,
    ) -> CandidateBoxState {
        build_common_candidate_box_state(
            &self.core.engine,
            CandidateBoxBuildOptions {
                mode: self.core.mode,
                is_active,
                grid_state: self.core.grid_state,
                expanded_card: Some(self.core.expanded_card),
                inline_preedit,
                voice_manager: self.voice_manager.as_ref(),
            },
        )
    }
}
