//! 循环宿主：循环与外部运行时之间的通道
//!
//! 循环通过宿主发出事件、同步写入的消息、读取排队消息并等待审批答复。
//! [`crate::Agent`] 是内置宿主；直接使用低层循环时可自行实现。

use crate::event::AgentEvent;
use async_trait::async_trait;
use serde_json::Value;
use zach_ai_core::Message;

/// 待人工审批的工具调用
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    /// 审批请求 ID
    pub approval_id: String,
    /// 关联的工具调用 ID
    pub tool_call_id: String,
    /// 工具名称
    pub tool_name: String,
    /// 工具入参
    pub input: Value,
    /// 是否为厂商侧执行的工具
    pub provider_executed: bool,
}

/// 审批答复
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalDecision {
    /// 是否批准
    pub approved: bool,
    /// 批注或拒绝原因
    pub reason: Option<String>,
}

impl ApprovalDecision {
    /// 批准
    pub fn approve() -> Self {
        Self {
            approved: true,
            reason: None,
        }
    }

    /// 拒绝
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            approved: false,
            reason: Some(reason.into()),
        }
    }
}

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

    /// 等待审批答复。运行被中止时该等待会被丢弃并按拒绝处理。
    async fn wait_approval(&self, request: ApprovalRequest) -> ApprovalDecision {
        let _ = request;
        ApprovalDecision::deny("未配置审批通道")
    }
}
