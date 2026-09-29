//! 工具审批：请求与答复的数据模型
//!
//! 循环在工具执行前把需要人工确认的调用封装为 [`ApprovalRequest`]，交给宿主
//! （见 [`crate::LoopHost::wait_approval`]）取得 [`ApprovalDecision`] 后再决定是否执行。

use serde_json::Value;

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
