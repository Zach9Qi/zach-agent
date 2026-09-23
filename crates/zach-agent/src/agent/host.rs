//! `Agent` 内置的循环宿主：事件转发、消息镜像、队列读取与审批等待

use super::state::Inner;
use crate::event::AgentEvent;
use crate::host::{ApprovalDecision, ApprovalRequest, LoopHost};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use zach_ai_core::Message;

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
