//! 单步模型调用：构造请求、消费流并透出事件、聚合结果，失败时按策略重试

use super::tools::{approval_request_for, validate_call};
use crate::config::RetryPolicy;
use crate::context::{AgentContext, RequestState};
use crate::event::{map_stream_part, AgentEvent};
use crate::hooks::AgentHooks;
use crate::host::LoopHost;
use crate::utils::cancellable;
use futures::StreamExt;
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;
use zach_ai_core::response::content::parse_tool_input;
use zach_ai_core::{
    GenerateResult, Message, ModelError, OutputContent, Prompt, StreamAccumulator, StreamPart,
    UnifiedFinishReason,
};

/// 单步结果
pub(super) enum StepOutcome {
    /// 模型正常收尾
    Completed(GenerateResult),
    /// 运行被中止，附带已聚合的半成品（若已开始输出）
    Aborted(Option<GenerateResult>),
    /// 重试用尽或不可重试
    Failed(ModelError),
}

/// 单步执行所需的依赖
pub(super) struct StepInput<'a> {
    pub(super) state: &'a RequestState,
    pub(super) hooks: &'a dyn AgentHooks,
    pub(super) host: &'a dyn LoopHost,
    pub(super) retry: &'a RetryPolicy,
    pub(super) step_index: usize,
    pub(super) cancel: &'a CancellationToken,
}

/// 调用模型直到成功、中止或失败
pub(super) async fn run_step(input: StepInput<'_>) -> StepOutcome {
    let mut attempt = 0;
    loop {
        let error = match stream_once(&input).await {
            Ok(outcome) => return outcome,
            Err(error) => error,
        };
        if !error.is_retryable() || attempt >= input.retry.max_retries {
            return StepOutcome::Failed(error);
        }
        attempt += 1;
        input
            .host
            .emit(AgentEvent::StepRetry {
                step_index: input.step_index,
                reason: Some(error.to_string()),
            })
            .await;
        let delay = input.retry.delay_for(attempt);
        if cancellable(input.cancel, tokio::time::sleep(delay))
            .await
            .is_none()
        {
            return StepOutcome::Aborted(None);
        }
    }
}

/// 一次模型请求。`Err` 表示本次尝试失败，由调用方决定是否重试。
async fn stream_once(input: &StepInput<'_>) -> Result<StepOutcome, ModelError> {
    let StepInput {
        state,
        hooks,
        host,
        cancel,
        ..
    } = *input;

    let Some(messages) = cancellable(
        cancel,
        hooks.transform_context(state.context.messages.clone()),
    )
    .await
    else {
        return Ok(StepOutcome::Aborted(None));
    };
    let mut options = state.options.clone();
    options.prompt = build_prompt(&state.context, messages);
    options.tools = state.context.tool_definitions();

    let mut stream = match cancellable(cancel, state.model.do_stream(options)).await {
        None => return Ok(StepOutcome::Aborted(None)),
        Some(stream) => stream?,
    };

    let mut accumulator = StreamAccumulator::new();
    let mut announced = HashSet::new();
    loop {
        let item = match cancellable(cancel, stream.next()).await {
            None => return Ok(StepOutcome::Aborted(Some(accumulator.finish()))),
            Some(None) => break,
            Some(Some(item)) => item?,
        };
        if let Some(event) = map_stream_part(&item) {
            host.emit(event).await;
        }
        let trigger = Trigger::of(&item);
        accumulator.process(item);
        match trigger {
            Trigger::ToolCall(id) => {
                announce(
                    &id,
                    accumulator.content(),
                    &state.context,
                    &mut announced,
                    host,
                )
                .await
            }
            Trigger::Approval(approval_id, tool_call_id) => {
                emit_provider_approval(&approval_id, &tool_call_id, accumulator.content(), host)
                    .await
            }
            Trigger::None => {}
        }
    }

    let result = accumulator.finish();
    let ids: Vec<String> = result
        .content
        .iter()
        .filter_map(|part| match part {
            OutputContent::ToolCall { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect();
    for id in ids {
        announce(&id, &result.content, &state.context, &mut announced, host).await;
    }

    match result.finish_reason.unified {
        UnifiedFinishReason::Unknown => Err(ModelError::StreamError {
            message: "模型流未正常收尾".to_string(),
            source: None,
        }),
        UnifiedFinishReason::Error => Err(ModelError::Other(
            result
                .finish_reason
                .raw
                .unwrap_or_else(|| "模型生成失败".to_string()),
        )),
        _ => Ok(StepOutcome::Completed(result)),
    }
}

fn build_prompt(context: &AgentContext, messages: Vec<Message>) -> Prompt {
    let mut prompt = Prompt::new();
    if let Some(system) = context.system_prompt.as_deref().filter(|s| !s.is_empty()) {
        prompt.push(Message::system(system));
    }
    prompt.messages.extend(messages);
    prompt
}

/// 聚合后需要结合上下文补发事件的分块
enum Trigger {
    ToolCall(String),
    Approval(String, String),
    None,
}

impl Trigger {
    fn of(part: &StreamPart) -> Self {
        match part {
            StreamPart::ToolCall { tool_call_id, .. } => Self::ToolCall(tool_call_id.clone()),
            StreamPart::ToolApprovalRequest {
                approval_id,
                tool_call_id,
                ..
            } => Self::Approval(approval_id.clone(), tool_call_id.clone()),
            _ => Self::None,
        }
    }
}

/// 工具入参就绪：本地工具先校验，发出 `ToolInputAvailable` 或 `ToolInputError`；每个调用只发一次
async fn announce(
    tool_call_id: &str,
    content: &[OutputContent],
    context: &AgentContext,
    announced: &mut HashSet<String>,
    host: &dyn LoopHost,
) {
    let Some(OutputContent::ToolCall {
        tool_name,
        input,
        provider_executed,
        dynamic,
        provider_metadata,
        ..
    }) = content.iter().find(|part| {
        matches!(part, OutputContent::ToolCall { tool_call_id: id, .. } if id == tool_call_id)
    })
    else {
        return;
    };
    if !announced.insert(tool_call_id.to_string()) {
        return;
    }

    let event = if *provider_executed {
        AgentEvent::ToolInputAvailable {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.clone(),
            input: parse_tool_input(input),
            dynamic: *dynamic,
            provider_executed: true,
            title: None,
            tool_metadata: None,
            provider_metadata: provider_metadata.clone(),
        }
    } else {
        match validate_call(context, tool_name, input) {
            Ok((tool, value)) => AgentEvent::ToolInputAvailable {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.clone(),
                input: value,
                dynamic: *dynamic,
                provider_executed: false,
                title: tool.title().map(str::to_string),
                tool_metadata: None,
                provider_metadata: provider_metadata.clone(),
            },
            Err(error_text) => AgentEvent::ToolInputError {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.clone(),
                input: input.clone(),
                error_text,
                dynamic: *dynamic,
                provider_executed: false,
                title: None,
                tool_metadata: None,
                provider_metadata: provider_metadata.clone(),
            },
        }
    };
    host.emit(event).await;
}

async fn emit_provider_approval(
    approval_id: &str,
    tool_call_id: &str,
    content: &[OutputContent],
    host: &dyn LoopHost,
) {
    let request = approval_request_for(content, approval_id, tool_call_id);
    host.emit(AgentEvent::ToolApprovalRequest {
        approval_id: request.approval_id,
        tool_call_id: request.tool_call_id,
        tool_name: request.tool_name,
        input: request.input,
        approval_descriptor: None,
        reason: None,
        is_automatic: None,
        signature: None,
    })
    .await;
}

/// 中止时保留已生成的内容，丢弃未完成的工具调用等需要配对的块
pub(super) fn partial_assistant(result: GenerateResult) -> Option<Message> {
    let parts: Vec<_> = result
        .content
        .into_iter()
        .filter(|part| {
            matches!(
                part,
                OutputContent::Text { .. }
                    | OutputContent::Reasoning { .. }
                    | OutputContent::File { .. }
                    | OutputContent::ReasoningFile { .. }
            )
        })
        .filter_map(OutputContent::into_assistant_part)
        .collect();
    (!parts.is_empty()).then(|| Message::Assistant {
        content: parts,
        provider_options: result.provider_metadata,
    })
}
