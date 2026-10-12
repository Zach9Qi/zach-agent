//! 会话占用 Agent 的钩子位：串联宿主钩子，并在写入失败后停止运行

use super::shared::Shared;
use async_trait::async_trait;
use std::sync::Arc;
use zach_agent::{
    AgentContext, AgentHooks, RequestState, ToolCallDecision, ToolCallInfo, ToolOutcome,
    TurnDecision, TurnInfo,
};
use zach_ai_core::Message;

pub(super) struct SessionHooks {
    pub(super) shared: Arc<Shared>,
    pub(super) inner: Arc<dyn AgentHooks>,
}

#[async_trait]
impl AgentHooks for SessionHooks {
    async fn transform_context(&self, messages: Vec<Message>) -> Vec<Message> {
        self.inner.transform_context(messages).await
    }

    async fn prepare_request(&self, request: &mut RequestState) {
        self.inner.prepare_request(request).await;
    }

    /// 会话无法写入时不再执行任何工具：对话记录缺失时继续改动工作区会让两者对不上
    async fn before_tool_call(&self, call: &ToolCallInfo<'_>) -> ToolCallDecision {
        if let Some(failure) = self.shared.failure() {
            return ToolCallDecision::Deny {
                reason: Some(format!("会话写入失败，已停止执行工具：{failure}")),
                terminate: true,
            };
        }
        self.inner.before_tool_call(call).await
    }

    async fn after_tool_call(&self, call: &ToolCallInfo<'_>, outcome: ToolOutcome) -> ToolOutcome {
        self.inner.after_tool_call(call, outcome).await
    }

    async fn finish_turn(&self, turn: &TurnInfo, context: &AgentContext) -> TurnDecision {
        if self.shared.failure().is_some() {
            return TurnDecision::End;
        }
        self.inner.finish_turn(turn, context).await
    }

    async fn prepare_next_turn(&self, turn: &TurnInfo, request: &mut RequestState) -> Vec<Message> {
        self.inner.prepare_next_turn(turn, request).await
    }
}
