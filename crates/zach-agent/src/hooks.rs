//! 策略钩子：在循环的关键节点介入上下文、请求、工具调用与轮次调度
//!
//! 钩子不返回错误，需要自行给出安全的兜底值。运行被中止时，正在等待的钩子会被直接丢弃。

use crate::context::{AgentContext, RequestState};
use crate::tool::ToolOutcome;
use async_trait::async_trait;
use serde_json::Value;
use zach_ai_core::{FinishReason, Message, Usage};

/// 工具调用信息，传给工具相关钩子
#[derive(Debug, Clone, Copy)]
pub struct ToolCallInfo<'a> {
    /// 工具调用 ID
    pub tool_call_id: &'a str,
    /// 工具名称
    pub tool_name: &'a str,
    /// 已通过 [`crate::AgentTool::prepare_input`] 的入参
    pub input: &'a Value,
    /// 发出该调用的助手消息
    pub assistant: &'a Message,
    /// 当前上下文
    pub context: &'a AgentContext,
}

/// [`AgentHooks::before_tool_call`] 的决策
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ToolCallDecision {
    /// 放行（仍会遵循工具自身的审批要求）
    #[default]
    Allow,
    /// 拒绝执行，模型收到 `execution_denied` 结果
    Deny {
        reason: Option<String>,
        /// 参与"整批终止"判定
        terminate: bool,
    },
    /// 需要人工审批后才能执行
    RequireApproval {
        reason: Option<String>,
        /// 审批上下文描述（风险评级、差异预览等）
        descriptor: Option<Value>,
    },
}

impl ToolCallDecision {
    /// 拒绝执行
    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Deny {
            reason: Some(reason.into()),
            terminate: false,
        }
    }

    /// 要求人工审批
    pub fn require_approval(reason: impl Into<String>) -> Self {
        Self::RequireApproval {
            reason: Some(reason.into()),
            descriptor: None,
        }
    }
}

/// 一个已完成轮次的摘要
#[derive(Debug, Clone, PartialEq)]
pub struct TurnInfo {
    /// 轮次索引（从 0 开始）
    pub step_index: usize,
    /// 本轮助手消息
    pub assistant: Message,
    /// 本轮工具结果消息（没有工具调用时为 `None`）
    pub tool_results: Option<Message>,
    /// 模型结束原因
    pub finish_reason: FinishReason,
    /// 本轮模型用量
    pub usage: Usage,
}

/// [`AgentHooks::finish_turn`] 的调度决策
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TurnDecision {
    /// 按常规调度：有工具结果或排队消息就继续，否则结束
    #[default]
    Default,
    /// 保证至少再发起一次模型请求
    Continue,
    /// 立即结束运行，不再读取排队消息
    End,
}

/// Agent 策略钩子，所有方法都有空实现
#[async_trait]
pub trait AgentHooks: Send + Sync {
    /// 每次请求前对对话记录做变换（裁剪、注入外部上下文）。结果只用于本次请求，不写回。
    async fn transform_context(&self, messages: Vec<Message>) -> Vec<Message> {
        messages
    }

    /// 每次请求前调用（含首次），可替换模型、调用参数与上下文
    async fn prepare_request(&self, request: &mut RequestState) {
        let _ = request;
    }

    /// 工具入参校验通过后、执行前调用
    async fn before_tool_call(&self, call: &ToolCallInfo<'_>) -> ToolCallDecision {
        let _ = call;
        ToolCallDecision::Allow
    }

    /// 工具执行后调用，可改写结果
    async fn after_tool_call(&self, call: &ToolCallInfo<'_>, outcome: ToolOutcome) -> ToolOutcome {
        let _ = call;
        outcome
    }

    /// 轮次完成（助手消息与工具结果都已写入）后调用，决定是否继续
    async fn finish_turn(&self, turn: &TurnInfo, context: &AgentContext) -> TurnDecision {
        let _ = (turn, context);
        TurnDecision::Default
    }

    /// 确定继续后、下一轮开始前调用。可修改运行态，返回的消息会在下一轮请求前追加。
    async fn prepare_next_turn(&self, turn: &TurnInfo, request: &mut RequestState) -> Vec<Message> {
        let _ = (turn, request);
        Vec::new()
    }
}

/// 不做任何干预的钩子
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopHooks;

impl AgentHooks for NoopHooks {}
