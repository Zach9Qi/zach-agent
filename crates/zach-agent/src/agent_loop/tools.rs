//! 工具批次：执行一条助手消息中的全部本地工具调用，并答复厂商侧审批请求

mod execute;
mod prepare;

pub(crate) use prepare::validate_call;

use crate::context::AgentContext;
use crate::event::AgentEvent;
use crate::hooks::AgentHooks;
use crate::host::{ApprovalDecision, ApprovalRequest, LoopHost};
use crate::tool::{ToolExecutionMode, ToolOutcome};
use crate::utils::cancellable;
use futures::future::join_all;
use prepare::{Prepared, ABORTED};
use tokio_util::sync::CancellationToken;
use zach_ai_core::response::content::parse_tool_input;
use zach_ai_core::{Message, OutputContent, ToolPart};

/// 需要本地执行的工具调用
pub(crate) struct LocalCall {
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) raw_input: String,
}

/// 批次执行所需的上下文
#[derive(Clone, Copy)]
pub(crate) struct BatchInput<'a> {
    pub(crate) assistant: &'a Message,
    pub(crate) content: &'a [OutputContent],
    pub(crate) context: &'a AgentContext,
    pub(crate) hooks: &'a dyn AgentHooks,
    pub(crate) host: &'a dyn LoopHost,
    pub(crate) mode: ToolExecutionMode,
    pub(crate) cancel: &'a CancellationToken,
    /// 模型输出因长度上限被截断，工具入参可能残缺
    pub(crate) truncated: bool,
}

/// 批次执行结果
pub(crate) struct ToolBatch {
    /// 待写入对话记录的工具消息
    pub(crate) message: Option<Message>,
    /// 本批是否包含需要回传给模型的调用
    pub(crate) has_calls: bool,
    /// 所有本地工具都请求终止
    pub(crate) terminate: bool,
}

/// 执行整批工具调用
pub(crate) async fn run_tool_batch(batch: BatchInput<'_>) -> ToolBatch {
    let calls = local_calls(batch.content);
    let approvals = provider_approvals(batch.content);
    if calls.is_empty() && approvals.is_empty() {
        return ToolBatch {
            message: None,
            has_calls: false,
            terminate: false,
        };
    }

    let outcomes = if batch.truncated {
        fail_truncated(&calls, &batch).await
    } else if is_sequential(&calls, &batch) {
        run_sequential(&calls, &batch).await
    } else {
        run_parallel(&calls, &batch).await
    };
    let terminate = !outcomes.is_empty() && outcomes.iter().all(|outcome| outcome.terminate);

    let mut parts: Vec<ToolPart> = calls
        .iter()
        .zip(outcomes)
        .map(|(call, outcome)| ToolPart::ToolResult {
            tool_call_id: call.tool_call_id.clone(),
            tool_name: call.tool_name.clone(),
            output: outcome.output,
            provider_options: None,
        })
        .collect();
    parts.extend(answer_provider_approvals(approvals, &batch).await);

    ToolBatch {
        message: Some(Message::tool(parts)),
        has_calls: true,
        terminate,
    }
}

/// 为厂商侧审批请求补齐工具名与入参
pub(crate) fn approval_request_for(
    content: &[OutputContent],
    approval_id: &str,
    tool_call_id: &str,
) -> ApprovalRequest {
    let call = content.iter().find_map(|part| match part {
        OutputContent::ToolCall {
            tool_call_id: id,
            tool_name,
            input,
            ..
        } if id == tool_call_id => Some((tool_name.clone(), parse_tool_input(input))),
        _ => None,
    });
    let (tool_name, input) = call.unwrap_or_default();
    ApprovalRequest {
        approval_id: approval_id.to_string(),
        tool_call_id: tool_call_id.to_string(),
        tool_name,
        input,
        provider_executed: true,
    }
}

fn local_calls(content: &[OutputContent]) -> Vec<LocalCall> {
    content
        .iter()
        .filter_map(|part| match part {
            OutputContent::ToolCall {
                tool_call_id,
                tool_name,
                input,
                provider_executed: false,
                ..
            } => Some(LocalCall {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                raw_input: input.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn provider_approvals(content: &[OutputContent]) -> Vec<ApprovalRequest> {
    content
        .iter()
        .filter_map(|part| match part {
            OutputContent::ToolApprovalRequest {
                approval_id,
                tool_call_id,
                ..
            } => Some(approval_request_for(content, approval_id, tool_call_id)),
            _ => None,
        })
        .collect()
}

fn is_sequential(calls: &[LocalCall], batch: &BatchInput<'_>) -> bool {
    batch.mode == ToolExecutionMode::Sequential
        || calls.iter().any(|call| {
            batch
                .context
                .find_tool(&call.tool_name)
                .and_then(|tool| tool.execution_mode())
                == Some(ToolExecutionMode::Sequential)
        })
}

async fn fail_truncated(calls: &[LocalCall], batch: &BatchInput<'_>) -> Vec<ToolOutcome> {
    let mut outcomes = Vec::with_capacity(calls.len());
    for call in calls {
        let message = format!(
            "工具 {} 未执行：模型输出达到长度上限，入参可能被截断，请重新发起完整的调用",
            call.tool_name
        );
        outcomes.push(prepare::settle(call, ToolOutcome::error(message), batch).await);
    }
    outcomes
}

async fn run_sequential(calls: &[LocalCall], batch: &BatchInput<'_>) -> Vec<ToolOutcome> {
    let mut outcomes = Vec::with_capacity(calls.len());
    for call in calls {
        let outcome = if batch.cancel.is_cancelled() {
            prepare::aborted(call, batch).await
        } else {
            match prepare::prepare(call, batch).await {
                Prepared::Done(outcome) => outcome,
                Prepared::Ready { tool, input } => execute::execute(call, tool, input, batch).await,
            }
        };
        outcomes.push(outcome);
    }
    outcomes
}

/// 依次准备（含审批），放行的调用并发执行；结果按调用顺序返回
async fn run_parallel(calls: &[LocalCall], batch: &BatchInput<'_>) -> Vec<ToolOutcome> {
    let mut prepared = Vec::with_capacity(calls.len());
    for call in calls {
        if batch.cancel.is_cancelled() {
            prepared.push(Prepared::Done(prepare::aborted(call, batch).await));
        } else {
            prepared.push(prepare::prepare(call, batch).await);
        }
    }
    let runs = calls
        .iter()
        .zip(prepared)
        .map(|(call, prepared)| async move {
            match prepared {
                Prepared::Done(outcome) => outcome,
                Prepared::Ready { tool, input } => execute::execute(call, tool, input, batch).await,
            }
        });
    join_all(runs).await
}

async fn answer_provider_approvals(
    approvals: Vec<ApprovalRequest>,
    batch: &BatchInput<'_>,
) -> Vec<ToolPart> {
    let mut parts = Vec::with_capacity(approvals.len());
    for request in approvals {
        let approval_id = request.approval_id.clone();
        let decision = cancellable(batch.cancel, batch.host.wait_approval(request))
            .await
            .unwrap_or_else(|| ApprovalDecision::deny(ABORTED));
        batch
            .host
            .emit(AgentEvent::ToolApprovalResponse {
                approval_id: approval_id.clone(),
                approved: decision.approved,
                reason: decision.reason.clone(),
                provider_executed: true,
                provider_metadata: None,
            })
            .await;
        parts.push(ToolPart::ToolApprovalResponse {
            approval_id,
            approved: decision.approved,
            reason: decision.reason,
            provider_options: None,
        });
    }
    parts
}
