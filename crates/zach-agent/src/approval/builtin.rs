//! 内置审批处理器

use super::{ApprovalHandler, ApprovalRequest, ApprovalRoute};
use async_trait::async_trait;
use std::time::Duration;

/// 交互式审批（默认）：每个请求都转交事件流，由 [`crate::Agent::respond_approval`] 答复
///
/// 未消费事件流（如调用 `outcome()` 或丢弃 `AgentRun`）时请求会被自动拒绝，
/// 无人值守场景请改用 [`DenyAll`]、[`ApproveAll`] 或自定义处理器。
#[derive(Debug, Clone, Default)]
pub struct InteractiveApproval {
    timeout: Option<Duration>,
}

impl InteractiveApproval {
    /// 永不超时
    pub fn new() -> Self {
        Self::default()
    }

    /// 超过 `timeout` 未答复则拒绝
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
        }
    }
}

#[async_trait]
impl ApprovalHandler for InteractiveApproval {
    async fn route(&self, _request: &ApprovalRequest) -> ApprovalRoute {
        ApprovalRoute::Ask {
            timeout: self.timeout,
        }
    }
}

/// 一律拒绝：适合批处理等无人值守场景，需审批的工具一概不执行
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAll;

#[async_trait]
impl ApprovalHandler for DenyAll {
    async fn route(&self, _request: &ApprovalRequest) -> ApprovalRoute {
        ApprovalRoute::deny("审批策略不允许执行需审批的工具")
    }
}

/// 一律批准
///
/// **危险**：等同于关闭审批，只应在沙箱、测试或工具本身无副作用的场景使用。
#[derive(Debug, Clone, Copy, Default)]
pub struct ApproveAll;

#[async_trait]
impl ApprovalHandler for ApproveAll {
    async fn route(&self, _request: &ApprovalRequest) -> ApprovalRoute {
        ApprovalRoute::approve()
    }
}
