//! 工具审批：请求/答复的数据模型与可插拔的审批处理器
//!
//! 循环在工具执行前把需要人工确认的调用封装为 [`ApprovalRequest`]，交给宿主
//! （见 [`crate::LoopHost::wait_approval`]）取得 [`ApprovalDecision`] 后再决定是否执行。
//!
//! [`crate::Agent`] 通过 [`ApprovalHandler`] 决定每个请求由谁裁决：处理器可以直接给出结论
//! （自动放行/拒绝），也可以转交交互通道——发出 `ToolApprovalRequest` 事件并等待
//! [`crate::Agent::respond_approval`]。这让"白名单自动通过、其余转人工"这类组合策略
//! 无需接触事件通道内部即可实现。

mod builtin;

pub use builtin::{ApproveAll, DenyAll, InteractiveApproval};

use crate::event::AgentEvent;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

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
    /// 触发审批的原因说明（来自钩子 `RequireApproval`）
    pub reason: Option<String>,
    /// 审批上下文描述（风险评级、差异预览等，来自钩子 `RequireApproval`）
    pub descriptor: Option<Value>,
}

impl ApprovalRequest {
    /// 转为呈现给使用者的 `ToolApprovalRequest` 事件
    ///
    /// 宿主在把请求转交人工时应发出该事件（内置宿主已处理；自定义 [`crate::LoopHost`] 可直接使用）。
    pub fn to_event(&self) -> AgentEvent {
        AgentEvent::ToolApprovalRequest {
            approval_id: self.approval_id.clone(),
            tool_call_id: self.tool_call_id.clone(),
            tool_name: self.tool_name.clone(),
            input: self.input.clone(),
            approval_descriptor: self.descriptor.clone(),
            reason: self.reason.clone(),
            is_automatic: None,
            signature: None,
        }
    }
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

/// 审批处理器给出的裁决路径
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalRoute {
    /// 直接给出结论，不打扰使用者
    Decided(ApprovalDecision),
    /// 转交交互通道：发出 `ToolApprovalRequest` 事件，等待 [`crate::Agent::respond_approval`]
    Ask {
        /// 等待答复的时限；`None` 表示永不超时。超时按拒绝处理。
        timeout: Option<Duration>,
    },
}

impl ApprovalRoute {
    /// 自动批准
    pub fn approve() -> Self {
        Self::Decided(ApprovalDecision::approve())
    }

    /// 自动拒绝
    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Decided(ApprovalDecision::deny(reason))
    }

    /// 转人工，永不超时
    pub fn ask() -> Self {
        Self::Ask { timeout: None }
    }

    /// 转人工，超过 `timeout` 未答复则拒绝
    pub fn ask_within(timeout: Duration) -> Self {
        Self::Ask {
            timeout: Some(timeout),
        }
    }
}

/// 审批处理器：决定一个审批请求由谁裁决
///
/// 实现方不返回错误，无法判断时应给出保守的兜底（通常是 [`ApprovalRoute::ask`] 或拒绝）。
/// 同步闭包 `Fn(&ApprovalRequest) -> ApprovalRoute` 可直接作为处理器使用。
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// 为请求选择裁决路径
    async fn route(&self, request: &ApprovalRequest) -> ApprovalRoute;
}

#[async_trait]
impl<F> ApprovalHandler for F
where
    F: Fn(&ApprovalRequest) -> ApprovalRoute + Send + Sync,
{
    async fn route(&self, request: &ApprovalRequest) -> ApprovalRoute {
        self(request)
    }
}
