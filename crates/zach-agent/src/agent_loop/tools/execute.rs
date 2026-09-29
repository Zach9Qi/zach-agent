//! 已放行工具调用的执行与收尾
//!
//! 工具是外部代码，其 panic 会被捕获并转为错误结果回传给模型，
//! 不会拖垮整个运行，也保证每个工具调用都有对应结果。

use super::prepare::{settle, ABORTED};
use super::{BatchInput, LocalCall};
use crate::event::tool_output_event;
use crate::hooks::ToolCallInfo;
use crate::tool::{SharedTool, ToolContext, ToolOutcome};
use crate::utils::{cancellable, panic_message};
use futures::FutureExt;
use serde_json::Value;
use std::panic::AssertUnwindSafe;
use tokio::sync::mpsc;
use zach_ai_core::ToolResultOutput;

/// 执行工具、透出中间态进度、应用执行后钩子并发出最终输出事件
pub(super) async fn execute(
    call: &LocalCall,
    tool: &SharedTool,
    input: Value,
    batch: &BatchInput<'_>,
) -> ToolOutcome {
    let (sender, mut progress) = mpsc::unbounded_channel();
    let ctx =
        ToolContext::new(call.tool_call_id.clone(), batch.cancel.clone()).with_progress(sender);
    let run = AssertUnwindSafe(tool.execute(input.clone(), ctx)).catch_unwind();
    tokio::pin!(run);

    let result = loop {
        tokio::select! {
            biased;
            _ = batch.cancel.cancelled() => break None,
            Some(update) = progress.recv() => emit_progress(call, update, batch).await,
            result = &mut run => break Some(result),
        }
    };
    while let Ok(update) = progress.try_recv() {
        emit_progress(call, update, batch).await;
    }
    drop(progress);

    let outcome = match result {
        None => return settle(call, ToolOutcome::error(ABORTED), batch).await,
        Some(Ok(Ok(outcome))) => outcome,
        Some(Ok(Err(error))) => ToolOutcome::error(error.to_string()),
        Some(Err(payload)) => {
            let message = panic_message(payload.as_ref());
            tracing::error!(tool = %call.tool_name, %message, "工具执行时 panic");
            ToolOutcome::error(format!("工具 {} 执行时 panic: {message}", call.tool_name))
        }
    };

    let info = ToolCallInfo {
        tool_call_id: &call.tool_call_id,
        tool_name: &call.tool_name,
        input: &input,
        assistant: batch.assistant,
        context: batch.context,
    };
    let outcome = cancellable(
        batch.cancel,
        batch.hooks.after_tool_call(&info, outcome.clone()),
    )
    .await
    .unwrap_or(outcome);
    settle(call, outcome, batch).await
}

async fn emit_progress(call: &LocalCall, update: ToolResultOutput, batch: &BatchInput<'_>) {
    let event = tool_output_event(call.tool_call_id.clone(), update, true, false, None);
    batch.host.emit(event).await;
}
