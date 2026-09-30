//! 示例：接入自定义模型，跑通一次"提问 → 工具调用 → 回答"的完整循环
//!
//! 运行：`cargo run -p zach-agent --example custom_model`
//!
//! 本示例不访问网络。[`RuleBasedModel`] 实现了 [`LanguageModel`]，用固定规则
//! 模拟一个"会调用工具"的模型：
//! 1. 第一轮看到用户消息 `a + b`，发起 `add` 工具调用；
//! 2. 第二轮看到工具结果，用文本作答。
//!
//! 接入真实厂商时只需替换模型实现，Agent、工具与事件消费代码无需改动。

use async_trait::async_trait;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use zach_agent::{typed_tool, Agent, AgentEvent, ToolContext, ToolError, ToolOutcome, TypedTool};
use zach_ai_core::{
    CallOptions, FinishReason, GenerateResult, LanguageModel, LanguageModelStream, Message,
    ModelError, StreamPart, ToolPart, ToolResultOutput, Usage, UserPart,
};

// ---------- 工具 ----------

/// 工具入参：派生 `JsonSchema` 后自动生成给模型看的参数声明
#[derive(Debug, Deserialize, JsonSchema)]
struct AddInput {
    /// 加数
    a: i64,
    /// 被加数
    b: i64,
}

struct AddTool;

#[async_trait]
impl TypedTool for AddTool {
    type Input = AddInput;

    fn name(&self) -> &str {
        "add"
    }

    fn description(&self) -> &str {
        "计算两个整数之和"
    }

    async fn call(&self, input: AddInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        Ok(ToolOutcome::json(json!({ "sum": input.a + input.b })))
    }
}

// ---------- 模型 ----------

/// 按固定规则产出流式分块的模型；真实实现会在这里发 HTTP 请求并解析 SSE
struct RuleBasedModel;

impl RuleBasedModel {
    /// 根据对话历史决定本轮输出
    fn plan(options: &CallOptions) -> Result<Vec<StreamPart>, ModelError> {
        match options.prompt.messages.last() {
            // 上一条是工具结果：读出 sum 并作答
            Some(Message::Tool { content, .. }) => {
                let sum = content
                    .iter()
                    .find_map(|part| match part {
                        ToolPart::ToolResult {
                            output: ToolResultOutput::Json { value, .. },
                            ..
                        } => value.get("sum").cloned(),
                        _ => None,
                    })
                    .unwrap_or_default();
                Ok(text_reply(format!("结果是 {sum}")))
            }
            // 上一条是用户提问：解析 "a + b" 并发起工具调用
            Some(Message::User { content, .. }) => {
                let question = content
                    .iter()
                    .find_map(|part| match part {
                        UserPart::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let (a, b) = question
                    .split_once('+')
                    .and_then(|(a, b)| {
                        Some((a.trim().parse::<i64>().ok()?, b.trim().parse::<i64>().ok()?))
                    })
                    .ok_or_else(|| ModelError::unsupported("prompt", Some("只会算加法".into())))?;
                Ok(vec![
                    StreamPart::ToolCall {
                        tool_call_id: "call_1".into(),
                        tool_name: "add".into(),
                        input: json!({ "a": a, "b": b }).to_string(),
                        provider_executed: false,
                        dynamic: false,
                        provider_metadata: None,
                    },
                    finish(FinishReason::tool_calls()),
                ])
            }
            _ => Err(ModelError::unsupported(
                "prompt",
                Some("对话历史为空".into()),
            )),
        }
    }
}

#[async_trait]
impl LanguageModel for RuleBasedModel {
    fn provider(&self) -> &str {
        "example"
    }

    fn model_id(&self) -> &str {
        "rule-based"
    }

    async fn do_generate(&self, _options: CallOptions) -> Result<GenerateResult, ModelError> {
        // Agent 循环只走流式接口；非流式可按需实现
        Err(ModelError::unsupported("generate", None))
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        let parts = Self::plan(&options)?;
        Ok(Box::pin(futures::stream::iter(parts.into_iter().map(Ok))))
    }
}

/// 一段完整的文本回复：Start → Delta → End → Finish
fn text_reply(text: String) -> Vec<StreamPart> {
    let id = "text_1".to_string();
    vec![
        StreamPart::TextStart {
            id: id.clone(),
            provider_metadata: None,
        },
        StreamPart::TextDelta {
            id: id.clone(),
            delta: text,
            provider_metadata: None,
        },
        StreamPart::TextEnd {
            id,
            provider_metadata: None,
        },
        finish(FinishReason::stop()),
    ]
}

fn finish(finish_reason: FinishReason) -> StreamPart {
    StreamPart::Finish {
        usage: Usage::simple(12, 6),
        finish_reason,
        provider_metadata: None,
    }
}

// ---------- 运行 ----------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = Agent::builder(Arc::new(RuleBasedModel))
        .system_prompt("你是一个只会调用 add 工具的计算助手")
        .tool(typed_tool(AddTool))
        .build();

    let mut run = agent.prompt_text("40 + 2")?;
    while let Some(event) = run.next().await {
        match event {
            AgentEvent::ToolInputAvailable {
                tool_name, input, ..
            } => println!("[调用工具] {tool_name}({input})"),
            AgentEvent::ToolOutputAvailable { output, .. } => println!("[工具返回] {output:?}"),
            AgentEvent::TextDelta { delta, .. } => println!("[模型回复] {delta}"),
            AgentEvent::RunFinish { .. } => println!("[运行结束]"),
            _ => {}
        }
    }

    println!("\n对话历史共 {} 条消息：", agent.messages().len());
    for message in agent.messages() {
        println!("  - {message:?}");
    }
    Ok(())
}
