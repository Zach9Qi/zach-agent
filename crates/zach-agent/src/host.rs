//! 循环宿主：循环与外部运行时之间的通道
//!
//! 循环通过宿主发出事件、同步写入的消息、读取排队消息并等待审批答复。
//! [`crate::Agent`] 是内置宿主；直接使用低层循环时可自行实现。

pub use crate::approval::{ApprovalDecision, ApprovalRequest};
use crate::event::AgentEvent;
use async_trait::async_trait;
use zach_ai_core::Message;

/// 循环宿主接口
#[async_trait]
pub trait LoopHost: Send + Sync {
    /// 发出运行时事件
    async fn emit(&self, event: AgentEvent);

    /// 一条消息写入了对话记录
    async fn on_message(&self, message: &Message) {
        let _ = message;
    }

    /// 取出插队消息（每轮结束后、以及运行开始时读取）
    async fn poll_steering(&self) -> Vec<Message> {
        Vec::new()
    }

    /// 取出追加消息（运行本应结束时读取）
    async fn poll_follow_up(&self) -> Vec<Message> {
        Vec::new()
    }

    /// 裁决一个审批请求
    ///
    /// 宿主负责把请求呈现给裁决方（转交人工时应发出 [`ApprovalRequest::to_event`] 事件，
    /// 自动裁决时无需发出）并返回结论；循环随后发出 `ToolApprovalResponse`。
    /// 运行被中止时该等待会被丢弃并按拒绝处理。
    async fn wait_approval(&self, request: ApprovalRequest) -> ApprovalDecision {
        let _ = request;
        ApprovalDecision::deny("未配置审批通道")
    }
}
