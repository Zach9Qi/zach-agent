//! 重试策略与上下文工具声明

use serde_json::json;
use std::time::Duration;
use zach_agent::{typed_tool, AgentContext, QueueMode, RetryPolicy, ToolContext, ToolError};
use zach_agent::{ToolOutcome, TypedTool};
use zach_ai_core::{ProviderTool, ToolDefinition};

#[test]
fn retry_delay_grows_exponentially_and_is_capped() {
    let policy = RetryPolicy {
        max_retries: 5,
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(350),
        multiplier: 2.0,
    };
    assert_eq!(policy.delay_for(1), Duration::from_millis(100));
    assert_eq!(policy.delay_for(2), Duration::from_millis(200));
    assert_eq!(policy.delay_for(3), Duration::from_millis(350));
}

#[test]
fn defaults_match_documented_behavior() {
    assert_eq!(RetryPolicy::default().max_retries, 2);
    assert_eq!(RetryPolicy::none().max_retries, 0);
    assert_eq!(QueueMode::default(), QueueMode::OneAtATime);
}

struct Echo;

#[async_trait::async_trait]
impl TypedTool for Echo {
    type Input = serde_json::Value;

    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "原样返回"
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        Ok(ToolOutcome::json(input))
    }
}

#[test]
fn context_declares_local_and_provider_tools() {
    let mut context = AgentContext::default();
    assert!(context.tool_definitions().is_none());

    context.tools.push(typed_tool(Echo));
    context.provider_tools.push(ProviderTool::new(
        "openai.web_search",
        "web_search",
        json!({}),
    ));

    let definitions = context.tool_definitions().unwrap();
    assert_eq!(definitions.len(), 2);
    assert!(matches!(&definitions[0], ToolDefinition::Function(f) if f.name == "echo"));
    assert!(definitions[1].is_provider());
    assert!(context.find_tool("echo").is_some());
    assert!(context.find_tool("missing").is_none());
}
