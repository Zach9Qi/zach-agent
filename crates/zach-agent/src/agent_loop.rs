//! 低层 Agent 循环
//!
//! 一个轮次（step）= 一次模型响应 + 执行其中的工具调用。循环在以下情况进入下一轮：
//! 本轮有工具结果、有插队消息、钩子要求继续；本应结束时若有追加消息，也会继续。

mod step;
mod tools;

use crate::config::LoopConfig;
use crate::context::{AgentContext, RequestState};
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::hooks::{AgentHooks, TurnDecision, TurnInfo};
use crate::host::LoopHost;
use crate::utils::{add_usage, cancellable, new_id};
use serde_json::json;
use std::sync::Arc;
use step::{partial_assistant, run_step, StepInput, StepOutcome};
use tokio_util::sync::CancellationToken;
use tools::{run_tool_batch, BatchInput};
use zach_ai_core::{FinishReason, Message, UnifiedFinishReason, Usage};

/// 一次运行的结果
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutput {
    /// 运行 ID（与 `RunStart` 事件一致）
    pub run_id: String,
    /// 本次运行写入对话记录的消息（含初始提示消息）
    pub messages: Vec<Message>,
    /// 各轮模型用量之和
    pub usage: Usage,
    /// 最后一轮的结束原因
    pub finish_reason: Option<FinishReason>,
    /// 完成的轮次数
    pub steps: usize,
    /// 是否被中止
    pub aborted: bool,
}

/// 以新的提示消息开始一次运行
///
/// 模型失败（重试用尽）时发出 `RunError` 并返回 `Err`；中止时发出 `RunAbort` 并返回
/// `aborted = true` 的结果。失败那一轮的半成品助手消息不会写入对话记录。
pub async fn run_agent_loop(
    prompts: Vec<Message>,
    context: AgentContext,
    config: LoopConfig,
    host: &dyn LoopHost,
    cancel: CancellationToken,
) -> Result<RunOutput, AgentError> {
    Runner::new(context, config, host, cancel)
        .run(prompts)
        .await
}

/// 从现有对话记录继续，不追加新消息（常用于失败后重试）
///
/// 最后一条消息必须是用户或工具消息。
pub async fn continue_agent_loop(
    context: AgentContext,
    config: LoopConfig,
    host: &dyn LoopHost,
    cancel: CancellationToken,
) -> Result<RunOutput, AgentError> {
    match context.messages.last() {
        None => return Err(AgentError::NoMessages),
        Some(Message::Assistant { .. }) => return Err(AgentError::CannotContinueFromAssistant),
        Some(_) => {}
    }
    run_agent_loop(Vec::new(), context, config, host, cancel).await
}

struct Runner<'a> {
    host: &'a dyn LoopHost,
    hooks: Arc<dyn AgentHooks>,
    config: LoopConfig,
    cancel: CancellationToken,
    state: RequestState,
    run_id: String,
    messages: Vec<Message>,
    usage: Usage,
    finish_reason: Option<FinishReason>,
    step_index: usize,
    steps: usize,
}

impl<'a> Runner<'a> {
    fn new(
        context: AgentContext,
        config: LoopConfig,
        host: &'a dyn LoopHost,
        cancel: CancellationToken,
    ) -> Self {
        let state = RequestState {
            model: config.model.clone(),
            options: config.options.clone(),
            context,
        };
        Self {
            host,
            hooks: config.hooks.clone(),
            config,
            cancel,
            state,
            run_id: new_id("run"),
            messages: Vec::new(),
            usage: Usage::default(),
            finish_reason: None,
            step_index: 0,
            steps: 0,
        }
    }

    async fn run(mut self, prompts: Vec<Message>) -> Result<RunOutput, AgentError> {
        self.host
            .emit(AgentEvent::RunStart {
                run_id: self.run_id.clone(),
                message_id: None,
                message_metadata: None,
            })
            .await;
        for message in prompts {
            self.commit(message).await;
        }

        let hooks = self.hooks.clone();
        let mut pending = self.host.poll_steering().await;
        let mut last_turn: Option<TurnInfo> = None;
        let mut explicit_continue = false;

        loop {
            let mut has_more_tool_calls = true;
            while has_more_tool_calls || !pending.is_empty() {
                let mut prepared = Vec::new();
                if let Some(turn) = &last_turn {
                    let next = hooks.prepare_next_turn(turn, &mut self.state);
                    let Some(messages) = cancellable(&self.cancel, next).await else {
                        return self.abort(false).await;
                    };
                    prepared = messages;
                    if pending.is_empty() {
                        pending = self.host.poll_steering().await;
                    }
                    self.step_index += 1;
                }

                let step_index = self.step_index;
                self.host.emit(AgentEvent::StepStart { step_index }).await;
                for message in prepared.into_iter().chain(pending.drain(..)) {
                    self.commit(message).await;
                }
                let prepare = hooks.prepare_request(&mut self.state);
                if cancellable(&self.cancel, prepare).await.is_none() {
                    return self.abort(true).await;
                }

                let result = match run_step(StepInput {
                    state: &self.state,
                    hooks: hooks.as_ref(),
                    host: self.host,
                    retry: &self.config.retry,
                    step_index,
                    cancel: &self.cancel,
                })
                .await
                {
                    StepOutcome::Completed(result) => result,
                    StepOutcome::Aborted(partial) => {
                        if let Some(message) = partial.and_then(partial_assistant) {
                            self.commit(message).await;
                        }
                        return self.abort(true).await;
                    }
                    StepOutcome::Failed(error) => {
                        let error_text = error.to_string();
                        self.host.emit(AgentEvent::RunError { error_text }).await;
                        return Err(error.into());
                    }
                };

                add_usage(&mut self.usage, &result.usage);
                self.finish_reason = Some(result.finish_reason.clone());
                self.steps += 1;
                let finish_reason = result.finish_reason.clone();
                let usage = result.usage.clone();
                let content = result.content.clone();
                let assistant = result.into_assistant_message();
                self.commit(assistant.clone()).await;

                let batch = run_tool_batch(BatchInput {
                    assistant: &assistant,
                    content: &content,
                    context: &self.state.context,
                    hooks: hooks.as_ref(),
                    host: self.host,
                    mode: self.config.tool_execution,
                    cancel: &self.cancel,
                    truncated: finish_reason.unified == UnifiedFinishReason::Length,
                })
                .await;
                if let Some(message) = &batch.message {
                    self.commit(message.clone()).await;
                }
                has_more_tool_calls = batch.has_calls && !batch.terminate;
                if self.cancel.is_cancelled() {
                    return self.abort(true).await;
                }

                let turn = TurnInfo {
                    step_index,
                    assistant,
                    tool_results: batch.message,
                    finish_reason,
                    usage,
                };
                let finish = hooks.finish_turn(&turn, &self.state.context);
                let Some(decision) = cancellable(&self.cancel, finish).await else {
                    return self.abort(true).await;
                };
                self.host.emit(AgentEvent::StepFinish { step_index }).await;
                last_turn = Some(turn);

                if decision == TurnDecision::End {
                    return Ok(self.finish().await);
                }
                explicit_continue = decision == TurnDecision::Continue;
                pending = self.host.poll_steering().await;
                if has_more_tool_calls || !pending.is_empty() {
                    explicit_continue = false;
                }
            }

            let follow_up = self.host.poll_follow_up().await;
            if !follow_up.is_empty() {
                explicit_continue = false;
                pending = follow_up;
                continue;
            }
            if explicit_continue {
                explicit_continue = false;
                continue;
            }
            break;
        }
        Ok(self.finish().await)
    }

    async fn commit(&mut self, message: Message) {
        self.host.on_message(&message).await;
        self.state.context.messages.push(message.clone());
        self.messages.push(message);
    }

    async fn abort(self, step_open: bool) -> Result<RunOutput, AgentError> {
        if step_open {
            let step_index = self.step_index;
            self.host.emit(AgentEvent::StepFinish { step_index }).await;
        }
        self.host
            .emit(AgentEvent::RunAbort {
                reason: Some("运行已中止".to_string()),
            })
            .await;
        Ok(self.into_output(true))
    }

    async fn finish(self) -> RunOutput {
        self.host
            .emit(AgentEvent::RunFinish {
                finish_reason: self.finish_reason.clone(),
                message_metadata: Some(json!({ "usage": self.usage })),
            })
            .await;
        self.into_output(false)
    }

    fn into_output(self, aborted: bool) -> RunOutput {
        RunOutput {
            run_id: self.run_id,
            messages: self.messages,
            usage: self.usage,
            finish_reason: self.finish_reason,
            steps: self.steps,
            aborted,
        }
    }
}
