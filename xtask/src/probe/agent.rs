//! 使用真实 Agent 循环验证本地工具执行和下一轮结果回传。

use super::{
    output::Reporter,
    scenario::{AgentSettings, Scenario},
    trace::Outcome,
    ProbeResult,
};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use zach_agent::{
    run_agent_loop, typed_tool, AgentContext, AgentEvent, AgentHooks, LoopConfig, LoopHost,
    RequestState, RetryPolicy, ToolContext, ToolError, ToolOutcome, TurnInfo, TypedTool,
};
use zach_ai_core::{AssistantPart, LanguageModel, Message, ToolChoice};

pub(super) async fn run(
    scenario: &Scenario,
    model: Arc<dyn LanguageModel>,
    reporter: Arc<Reporter>,
) -> ProbeResult<Outcome> {
    let cancel = CancellationToken::new();
    let host = Host {
        reporter: reporter.clone(),
        cancel: cancel.clone(),
    };
    let defaults = AgentSettings::default();
    let settings = scenario.agent.as_ref().unwrap_or(&defaults);
    let context = AgentContext {
        tools: settings.tools.iter().map(|_| typed_tool(Add)).collect(),
        ..Default::default()
    };
    let loop_config = LoopConfig::new(model)
        .with_options(scenario.request.clone())
        .with_hooks(Arc::new(AfterTool(settings.next_tool_choice.clone())))
        .with_retry(RetryPolicy::none());
    let output = run_agent_loop(
        scenario.request.prompt.messages.clone(),
        context,
        loop_config,
        &host,
        cancel,
    )
    .await
    .map_err(|e| e.to_string())?;
    reporter.emit("agent_result", &json!({
        "run_id": output.run_id, "steps": output.steps, "usage": output.usage,
        "finish_reason": output.finish_reason, "aborted": output.aborted, "messages": output.messages,
    }));
    if output.aborted {
        return Err("Agent 联调被中止".into());
    }
    let finish = output.finish_reason.ok_or("Agent 没有返回结束原因")?;
    let text = match output.messages.last() {
        Some(Message::Assistant { content, .. }) => content
            .iter()
            .filter_map(|part| match part {
                AssistantPart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    };
    Ok(Outcome {
        text,
        finish_reason: finish.unified,
        steps: output.steps,
        messages: output
            .messages
            .into_iter()
            .skip(scenario.request.prompt.len())
            .collect(),
    })
}

struct Host {
    reporter: Arc<Reporter>,
    cancel: CancellationToken,
}

#[async_trait]
impl LoopHost for Host {
    async fn emit(&self, event: AgentEvent) {
        self.reporter.emit("agent_event", &event);
        if self.reporter.check().is_err() {
            self.cancel.cancel();
        }
    }
}

struct AfterTool(Option<ToolChoice>);

#[async_trait]
impl AgentHooks for AfterTool {
    async fn prepare_next_turn(&self, _: &TurnInfo, request: &mut RequestState) -> Vec<Message> {
        if let Some(choice) = &self.0 {
            request.options.tool_choice = Some(choice.clone());
        }
        vec![]
    }
}

#[derive(Deserialize, JsonSchema)]
struct AddInput {
    /// 第一个整数
    a: i64,
    /// 第二个整数
    b: i64,
}

struct Add;

#[async_trait]
impl TypedTool for Add {
    type Input = AddInput;
    fn name(&self) -> &str {
        "add"
    }
    fn description(&self) -> &str {
        "计算两个整数之和"
    }
    async fn call(&self, input: AddInput, _: ToolContext) -> Result<ToolOutcome, ToolError> {
        let sum = input
            .a
            .checked_add(input.b)
            .ok_or_else(|| ToolError::failed("整数相加溢出"))?;
        Ok(ToolOutcome::json(json!({"sum": sum})))
    }
}
