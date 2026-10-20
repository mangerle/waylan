//! IBus 候选词列表鼠标点击与键盘翻页光标导航模块
//!
//! # 设计原理
//! - **实现初衷**：将原生候选词框的用户选词、翻页与上下导航事件从 D-Bus 接口总入口解耦。
//! - **核心优势**：在原子锁临界区内推进状态并记录词频/标点上下文，随后在锁外通过 `dispatch_outcome` 或 `emit_ui_snapshot` 异步同步界面。

use std::time::{SystemTime, UNIX_EPOCH};
use zbus::object_server::SignalContext;

use super::super::keys::{IbusKeyOutcome, IbusSessionCore};
use super::emitter::OutcomeDispatchParams;
use super::service::IbusEngineService;
use super::snapshot::UiSnapshot;

impl IbusEngineService {
    /// 响应用户在 GNOME 候选框上使用鼠标点击候选词事件
    pub(crate) async fn handle_candidate_clicked(&self, ctxt: &SignalContext<'_>, index: u32) {
        let (outcome, snap, is_caps_locked) = {
            let mut guard = self.core.lock();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let out = Self::apply_candidate_selection(&mut guard, index, now);
            let snap = UiSnapshot::capture(&guard);
            (out, Some(snap), guard.is_caps_locked())
        };
        self.dispatch_outcome(OutcomeDispatchParams {
            ctxt,
            outcome,
            snap,
            is_caps_locked,
        })
        .await;
    }

    /// 在原子锁临界区内完成候选词选词、标点上下文登记并产出按键结果
    ///
    /// # 设计原理
    /// - **实现初衷**：展开卡片态与普通单行态的选词入口不同（前者取全局索引并需先折叠卡片），
    ///   但两者都必须在同一把锁内推进状态，才能与词频记录和快照抓取保持原子一致。
    /// - **核心优势**：选词业务与快照/信号派发解耦，信号发射始终位于锁作用域之外。
    /// - **代价与局限**：选词失败（索引越界或组合已消费）时仅返回消费标记，不向调用方区分失败原因。
    fn apply_candidate_selection(
        guard: &mut IbusSessionCore,
        index: u32,
        now: u64,
    ) -> IbusKeyOutcome {
        let mut out = IbusKeyOutcome {
            consumed: true,
            update_ui: true,
            ..Default::default()
        };
        if guard.runtime.core.is_card_expanded() {
            let sel = guard.runtime.core.selected_global_index();
            guard.runtime.core.collapse_card();
            if let Some((committed, _)) = guard
                .runtime
                .core
                .engine
                .select_expanded_item(sel, index as usize)
            {
                guard
                    .runtime
                    .core
                    .punctuation_processor_mut()
                    .record_commit(&committed);
                out.commit_text = Some(committed);
            }
        } else if let Some((committed, _)) = guard
            .runtime
            .core
            .engine
            .select_candidate(index as usize, now)
        {
            guard
                .runtime
                .core
                .punctuation_processor_mut()
                .record_commit(&committed);
            out.commit_text = Some(committed);
        }
        out
    }

    /// 响应候选框上一页按钮事件
    pub(crate) async fn handle_page_up(&self, ctxt: &SignalContext<'_>) {
        let snap = {
            let mut guard = self.core.lock();
            if !guard.runtime.core.is_card_expanded() {
                let _ = guard.runtime.core.prev_page();
            }
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(ctxt, &snap).await;
    }

    /// 响应候选框下一页按钮事件
    pub(crate) async fn handle_page_down(&self, ctxt: &SignalContext<'_>) {
        let snap = {
            let mut guard = self.core.lock();
            if !guard.runtime.core.is_card_expanded() {
                let _ = guard.runtime.core.next_page();
            }
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(ctxt, &snap).await;
    }

    /// 响应候选框光标上移事件
    pub(crate) async fn handle_cursor_up(&self, ctxt: &SignalContext<'_>) {
        let snap = {
            let mut guard = self.core.lock();
            if guard.runtime.core.is_card_expanded() {
                guard.runtime.core.prev_card_item();
            } else {
                guard.runtime.core.prev_candidate();
            }
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(ctxt, &snap).await;
    }

    /// 响应候选框光标下移事件
    pub(crate) async fn handle_cursor_down(&self, ctxt: &SignalContext<'_>) {
        let snap = {
            let mut guard = self.core.lock();
            if guard.runtime.core.is_card_expanded() {
                guard.runtime.core.next_card_item();
            } else {
                guard.runtime.core.next_candidate();
            }
            UiSnapshot::capture(&guard)
        };
        Self::emit_ui_snapshot(ctxt, &snap).await;
    }
}
