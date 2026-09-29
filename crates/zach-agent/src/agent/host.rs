//! `Agent` 内置的循环宿主：事件转发、消息镜像、队列读取、审批等待与 panic 善后

use super::state::Inner;
use crate::event::AgentEvent;
use crate::host::{ApprovalDecision, ApprovalRequest, LoopHost};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use zach_ai_core::{AssistantPart, Message, ToolPart, ToolResultOutput};

pub(super) struct RunHost {
    inner: Arc<Inner>,
    events: mpsc::Sender<AgentEvent>,
    /// 首次读取插队消息时跳过（这些消息已作为本次运行的提示消息注入）
    skip_initial_steering: AtomicBool,
    /// 审批答复的接收端。发出请求事件前就登记，消费方收到事件后立即答复也不会丢失。
    replies: Mutex<HashMap<String, oneshot::Receiver<ApprovalDecision>>>,
}

impl RunHost {
    pub(super) fn new(
        inner: Arc<Inner>,
        events: mpsc::Sender<AgentEvent>,
        skip_initial_steering: bool,
    ) -> Self {
        Self {
            inner,
            events,
            skip_initial_steering: AtomicBool::new(skip_initial_steering),
            replies: Mutex::default(),
        }
    }

    fn register_approval(&self, approval_id: &str) {
        let (sender, receiver) = oneshot::channel();
        self.inner
            .lock()
            .approvals
            .insert(approval_id.to_string(), sender);
        self.replies_lock()
            .insert(approval_id.to_string(), receiver);
    }

    fn replies_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Receiver<ApprovalDecision>>> {
        self.replies
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        if let AgentEvent::ToolApprovalRequest { approval_id, .. } = &event {
            self.register_approval(approval_id);
        }
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
        if !self.replies_lock().contains_key(&request.approval_id) {
            self.register_approval(&request.approval_id);
        }
        let receiver = self.replies_lock().remove(&request.approval_id);
        let decision = match receiver {
            Some(receiver) => receiver.await.ok(),
            None => None,
        };
        self.inner.lock().approvals.remove(&request.approval_id);
        decision.unwrap_or_else(|| ApprovalDecision::deny("审批通道已关闭"))
    }
}
