//! 使用真实 Agent 循环验证本地工具执行和下一轮结果回传。

use super::{config::Config, output::Reporter, runner::require_stop, ProbeResult};
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
use zach_ai_core::{LanguageModel, Message, ToolChoice, ToolPart, ToolResultOutput};

pub(super) async fn run(
    config: &Config,
    model: Arc<dyn LanguageModel>,
    reporter: Arc<Reporter>,
) -> ProbeResult<()> {
    let cancel = CancellationToken::new();
    let host = Host {
        reporter: reporter.clone(),
        cancel: cancel.clone(),
    };
    let mut options = config.options.clone();
    options.tool_choice = Some(ToolChoice::specific("add"));
    let context = AgentContext {
        system_prompt: Some(
            "你是联调助手。先调用 add 工具，得到工具结果后给出简短文本答案。".into(),
        ),
        tools: vec![typed_tool(Add)],
        ..Default::default()
    };
    let loop_config = LoopConfig::new(model)
        .with_options(options)
        .with_hooks(Arc::new(AfterTool))
        .with_retry(RetryPolicy::none());
    let output = run_agent_loop(
        vec![Message::user(&config.prompt)],
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
    require_stop(finish.unified)?;
    let succeeded = output.messages.iter().any(|message| {
        let Message::Tool { content, .. } = message else {
            return false;
        };
        content.iter().any(|part| {
            matches!(part, ToolPart::ToolResult {
            tool_name, output: ToolResultOutput::Json { value, .. }, ..
        } if tool_name == "add" && value["sum"].as_i64().is_some())
        })
    });
    let answered = matches!(output.messages.last(),
        Some(Message::Assistant { content, .. }) if content.iter().any(|part|
            matches!(part, zach_ai_core::AssistantPart::Text { text, .. } if !text.trim().is_empty())));
    if !succeeded || !answered || output.steps < 2 {
        return Err("尚未验证完整工具链路：需要成功执行 add，并在下一轮收到模型文本回复".into());
    }
    Ok(())
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

struct AfterTool;

#[async_trait]
impl AgentHooks for AfterTool {
    async fn prepare_next_turn(&self, _: &TurnInfo, request: &mut RequestState) -> Vec<Message> {
        // 首轮强制 add；后续只要求回答，防止一直强制工具导致循环。
        request.options.tool_choice = Some(ToolChoice::None);
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
