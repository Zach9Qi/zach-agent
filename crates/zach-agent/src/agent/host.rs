//! `Agent` 内置的循环宿主：事件转发、消息镜像、队列读取、审批等待与 panic 善后

use super::state::Inner;
use crate::approval::{ApprovalDecision, ApprovalHandler, ApprovalRequest, ApprovalRoute};
use crate::event::AgentEvent;
use crate::host::LoopHost;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zach_ai_core::{AssistantPart, Message, ToolPart, ToolResultOutput};

pub(super) struct RunHost {
    inner: Arc<Inner>,
    events: mpsc::Sender<AgentEvent>,
    approval: Arc<dyn ApprovalHandler>,
    /// 首次读取插队消息时跳过（这些消息已作为本次运行的提示消息注入）
    skip_initial_steering: AtomicBool,
}

impl RunHost {
    pub(super) fn new(
        inner: Arc<Inner>,
        events: mpsc::Sender<AgentEvent>,
        approval: Arc<dyn ApprovalHandler>,
        skip_initial_steering: bool,
    ) -> Self {
        Self {
            inner,
            events,
            approval,
            skip_initial_steering: AtomicBool::new(skip_initial_steering),
        }
    }

    /// 交互路径：等待 `Agent::respond_approval` 的答复
    ///
    /// 先登记答复通道再发出请求事件，消费方收到事件后立即答复也不会丢失。
    /// 事件消费者（AgentRun）已丢弃时，审批请求注定无人应答：立即按拒绝处理，
    /// 避免运行永久悬挂、Agent 一直处于忙碌状态。超时同样按拒绝处理。
    async fn ask(&self, request: &ApprovalRequest, timeout: Option<Duration>) -> ApprovalDecision {
        let (sender, receiver) = oneshot::channel();
        self.inner
            .lock()
            .approvals
            .insert(request.approval_id.clone(), sender);
        self.emit(request.to_event()).await;

        let decision = tokio::select! {
            decision = receiver => decision.ok(),
            _ = self.events.closed() => None,
            _ = deadline(timeout) => Some(ApprovalDecision::deny(format!(
                "审批等待超时（{:?}）未得到答复", timeout.unwrap_or_default()
            ))),
        };
        self.inner.lock().approvals.remove(&request.approval_id);
        decision.unwrap_or_else(|| ApprovalDecision::deny("事件消费者已离开，审批无人应答"))
    }

    /// 运行任务 panic 后的善后：发出 `RunError`，并为悬空的工具调用补齐带有实际错误原因的结果，
    /// 保证对话记录仍能作为下一次请求的历史使用，模型也能知道调用为何没有结果
    pub(super) async fn recover_from_panic(&self, error_text: String) {
        let reason = format!("运行异常中断，工具未得出结果：{error_text}");
        self.emit(AgentEvent::RunError { error_text }).await;
        if let Some(repair) = self.dangling_tool_results(&reason) {
            self.on_message(&repair).await;
        }
    }

    /// 若对话记录末尾是带工具调用、但缺少对应结果的助手消息，则以 `reason` 为错误结果构造补齐用的工具消息
    ///
    /// panic 未必源自工具本身（也可能是钩子或宿主），因此措辞为"运行中断"而非"工具出错"。
    fn dangling_tool_results(&self, reason: &str) -> Option<Message> {
        let state = self.inner.lock();
        let Some(Message::Assistant { content, .. }) = state.messages.last() else {
            return None;
        };
        let parts: Vec<ToolPart> = content
            .iter()
            .filter_map(|part| match part {
                AssistantPart::ToolCall {
                    tool_call_id,
                    tool_name,
                    provider_executed: false,
                    ..
                } => Some(ToolPart::ToolResult {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    output: ToolResultOutput::error_text(reason),
                    provider_options: None,
                }),
                _ => None,
            })
            .collect();
        (!parts.is_empty()).then(|| Message::tool(parts))
    }
}

#[async_trait]
impl LoopHost for RunHost {
    async fn emit(&self, event: AgentEvent) {
        self.inner.lock().observe(&event);
        // 消费方已丢弃 AgentRun 时静默丢弃事件，运行继续
        let _ = self.events.send(event).await;
    }

    async fn on_message(&self, message: &Message) {
        self.inner.lock().messages.push(message.clone());
    }

    async fn poll_steering(&self) -> Vec<Message> {
        if self.skip_initial_steering.swap(false, Ordering::SeqCst) {
            return Vec::new();
        }
        self.inner.lock().steering.drain()
    }

    async fn poll_follow_up(&self) -> Vec<Message> {
        self.inner.lock().follow_up.drain()
    }

    async fn wait_approval(&self, request: ApprovalRequest) -> ApprovalDecision {
        match self.approval.route(&request).await {
            ApprovalRoute::Decided(decision) => decision,
            ApprovalRoute::Ask { timeout } => self.ask(&request, timeout).await,
        }
    }
}

/// 审批超时：`None` 表示永不超时
async fn deadline(timeout: Option<Duration>) {
    match timeout {
        Some(timeout) => tokio::time::sleep(timeout).await,
        None => std::future::pending().await,
    }
}
