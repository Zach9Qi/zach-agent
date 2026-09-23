//! 工具抽象与类型化工具适配

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use zach_agent::{typed_tool, ToolContext, ToolError, ToolOutcome, TypedTool};
use zach_ai_core::ToolResultOutput;

#[derive(Deserialize, JsonSchema)]
struct AddInput {
    /// 左操作数
    a: i64,
    /// 右操作数
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
        "两数相加"
    }

    fn needs_approval(&self, input: &AddInput) -> bool {
        input.a < 0
    }

    async fn call(&self, input: AddInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        Ok(ToolOutcome::text((input.a + input.b).to_string()))
    }
}

#[test]
fn typed_tool_generates_schema_from_input_type() {
    let tool = typed_tool(Add);
    let definition = tool.definition();
    assert_eq!(definition.name, "add");
    assert_eq!(definition.description.as_deref(), Some("两数相加"));

    let schema = &definition.input_schema;
    assert!(schema.get("$schema").is_none());
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"]["a"].is_object());
    assert_eq!(schema["required"], json!(["a", "b"]));
}

#[test]
fn typed_tool_rejects_invalid_input_before_execution() {
    let tool = typed_tool(Add);
    assert!(tool.prepare_input(json!({ "a": 1, "b": 2 })).is_ok());
    let err = tool.prepare_input(json!({ "a": "x" })).unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)));
}

#[test]
fn typed_tool_forwards_approval_decision() {
    let tool = typed_tool(Add);
    assert!(tool.needs_approval(&json!({ "a": -1, "b": 2 })));
    assert!(!tool.needs_approval(&json!({ "a": 1, "b": 2 })));
    assert!(!tool.needs_approval(&json!({ "bad": true })));
}

#[tokio::test]
async fn typed_tool_executes_with_deserialized_input() {
    let tool = typed_tool(Add);
    let ctx = ToolContext::new("call_1", CancellationToken::new());
    let outcome = tool.execute(json!({ "a": 2, "b": 3 }), ctx).await.unwrap();
    assert_eq!(outcome.output, ToolResultOutput::text("5"));
    assert!(!outcome.is_error());
    assert!(!outcome.terminate);
}

#[test]
fn outcome_error_flag_follows_output_variant() {
    assert!(ToolOutcome::error("失败").is_error());
    assert!(!ToolOutcome::json(json!({})).is_error());
    assert!(ToolOutcome::text("ok").with_terminate(true).terminate);
}
