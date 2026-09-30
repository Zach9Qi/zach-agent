//! `AgentEvent` 的 JSON 命名约定：判别值与字段名统一为 `snake_case`，与 `zach-ai-core` 一致

use serde_json::{json, Value};
use zach_agent::AgentEvent;
use zach_ai_core::ToolResultOutput;

fn encode(event: &AgentEvent) -> Value {
    serde_json::to_value(event).unwrap()
}

#[test]
fn type_tag_and_fields_are_snake_case() {
    let event = AgentEvent::ToolInputAvailable {
        tool_call_id: "call_1".to_string(),
        tool_name: "bash".to_string(),
        input: json!({ "cmd": "ls" }),
        dynamic: false,
        provider_executed: false,
        title: None,
        tool_metadata: None,
        provider_metadata: None,
    };
    let value = encode(&event);
    assert_eq!(value["type"], "tool_input_available");
    assert_eq!(value["tool_call_id"], "call_1");
    assert_eq!(value["tool_name"], "bash");
    assert_eq!(value["provider_executed"], false);
    assert!(value.get("toolCallId").is_none());

    assert_eq!(
        encode(&AgentEvent::RunStart {
            run_id: "run_1".to_string(),
            message_id: None,
            message_metadata: None,
        })["type"],
        "run_start"
    );
    assert_eq!(
        encode(&AgentEvent::StepRetry {
            step_index: 0,
            reason: None,
        })["type"],
        "step_retry"
    );
}

#[test]
fn roundtrip_and_reject_legacy_kebab_camel_naming() {
    let event = AgentEvent::ToolOutputAvailable {
        tool_call_id: "call_1".to_string(),
        output: ToolResultOutput::text("done"),
        preliminary: false,
        dynamic: false,
        provider_executed: false,
        tool_metadata: None,
        provider_metadata: None,
    };
    let encoded = serde_json::to_string(&event).unwrap();
    let decoded: AgentEvent = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, event);

    let legacy = r#"{"type":"tool-output-available","toolCallId":"call_1","output":{"type":"text","value":"done"}}"#;
    assert!(serde_json::from_str::<AgentEvent>(legacy).is_err());
}
