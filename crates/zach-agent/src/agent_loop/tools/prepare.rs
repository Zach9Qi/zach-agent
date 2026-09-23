//! 工具调用准备：查找工具、校验入参、执行前钩子与人工审批

use super::{BatchInput, LocalCall};
use crate::context::AgentContext;
use crate::event::{tool_output_event, AgentEvent};
use crate::hooks::{ToolCallDecision, ToolCallInfo};
use crate::host::ApprovalRequest;
use crate::tool::{SharedTool, ToolOutcome};
use crate::utils::{cancellable, new_id};
use serde_json::Value;
use zach_ai_core::ToolResultOutput;

pub(super) const ABORTED: &str = "操作已中止";

/// 准备结果：直接得出结果（不执行），或放行待执行
pub(super) enum Prepared<'a> {
    Done(ToolOutcome),
    Ready { tool: &'a SharedTool, input: Value },
}

/// 查找工具并把原始入参解析、校验为结构化 JSON
///
/// 空白入参视为 `{}`（无参工具在流式场景下的常态）。
pub(crate) fn validate_call<'a>(
    context: &'a AgentContext,
    tool_name: &str,
    raw_input: &str,
) -> Result<(&'a SharedTool, Value), String> {
    let tool = context
        .find_tool(tool_name)
        .ok_or_else(|| format!("工具 {tool_name} 不存在"))?;
    let trimmed = raw_input.trim();
    let value = if trimmed.is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(trimmed).map_err(|e| format!("工具入参不是合法 JSON: {e}"))?
    };
    let value = tool.prepare_input(value).map_err(|e| e.to_string())?;
    Ok((tool, value))
}

/// 准备单个调用。拒绝与中止会在这里发出事件；入参无效已在流阶段发过 `ToolInputError`，不再重复。
pub(super) async fn prepare<'a>(call: &LocalCall, batch: &BatchInput<'a>) -> Prepared<'a> {
    let (tool, input) = match validate_call(batch.context, &call.tool_name, &call.raw_input) {
        Ok(ready) => ready,
        Err(message) => return Prepared::Done(ToolOutcome::error(message)),
    };

    let info = ToolCallInfo {
        tool_call_id: &call.tool_call_id,
        tool_name: &call.tool_name,
        input: &input,
        assistant: batch.assistant,
        context: batch.context,
    };
    let Some(decision) = cancellable(batch.cancel, batch.hooks.before_tool_call(&info)).await
    else {
        return Prepared::Done(aborted(call, batch).await);
    };

    let approval = match decision {
        ToolCallDecision::Allow => tool.needs_approval(&input).then_some((None, None)),
        ToolCallDecision::RequireApproval { reason, descriptor } => Some((reason, descriptor)),
        ToolCallDecision::Deny { reason, terminate } => {
            let outcome =
                ToolOutcome::new(ToolResultOutput::denied(reason)).with_terminate(terminate);
            return Prepared::Done(settle(call, outcome, batch).await);
        }
    };

    if let Some((reason, descriptor)) = approval {
        match request_approval(call, &input, reason, descriptor, batch).await {
            None => return Prepared::Done(aborted(call, batch).await),
            Some((false, reason)) => {
                let outcome = ToolOutcome::new(ToolResultOutput::denied(reason));
                return Prepared::Done(settle(call, outcome, batch).await);
            }
            Some((true, _)) => {}
        }
    }

    if batch.cancel.is_cancelled() {
        return Prepared::Done(aborted(call, batch).await);
    }
    Prepared::Ready { tool, input }
}

/// 发出审批请求并等待答复；运行被中止时返回 `None`
async fn request_approval(
    call: &LocalCall,
    input: &Value,
    reason: Option<String>,
    descriptor: Option<Value>,
    batch: &BatchInput<'_>,
) -> Option<(bool, Option<String>)> {
    let approval_id = new_id("approval");
    batch
        .host
        .emit(AgentEvent::ToolApprovalRequest {
            approval_id: approval_id.clone(),
            tool_call_id: call.tool_call_id.clone(),
            tool_name: call.tool_name.clone(),
            input: input.clone(),
            approval_descriptor: descriptor,
            reason,
            is_automatic: None,
            signature: None,
        })
        .await;

    let request = ApprovalRequest {
        approval_id: approval_id.clone(),
        tool_call_id: call.tool_call_id.clone(),
        tool_name: call.tool_name.clone(),
        input: input.clone(),
        provider_executed: false,
    };
    let decision = cancellable(batch.cancel, batch.host.wait_approval(request)).await?;
    batch
        .host
        .emit(AgentEvent::ToolApprovalResponse {
            approval_id,
            approved: decision.approved,
            reason: decision.reason.clone(),
            provider_executed: false,
            provider_metadata: None,
        })
        .await;
    Some((decision.approved, decision.reason))
}

/// 调用得出最终结果：发出输出事件并返回结果
pub(super) async fn settle(
    call: &LocalCall,
    outcome: ToolOutcome,
    batch: &BatchInput<'_>,
) -> ToolOutcome {
    let event = tool_output_event(
        call.tool_call_id.clone(),
        outcome.output.clone(),
        false,
        false,
        None,
    );
    batch.host.emit(event).await;
    outcome
}

/// 运行被中止时补一条错误结果，保证每个工具调用都有对应结果
pub(super) async fn aborted(call: &LocalCall, batch: &BatchInput<'_>) -> ToolOutcome {
    settle(call, ToolOutcome::error(ABORTED), batch).await
}
