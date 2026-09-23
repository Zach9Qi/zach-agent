//! 枚举判别值命名：同一概念在流、输出、提示词与调用选项上共用 snake_case 标签。

use serde_json::{json, Value};
use zach_ai_core::{
    AssistantPart, OutputContent, ReasoningEffort, StreamPart, ToolChoice, ToolPart,
    ToolResultOutput, UnifiedFinishReason,
};

fn type_tag(value: &Value) -> &str {
    value["type"].as_str().expect("缺少 type 判别字段")
}

#[test]
fn shared_concepts_serialize_as_snake_case() {
    let stream_call = StreamPart::ToolCall {
        tool_call_id: "call_1".to_string(),
        tool_name: "bash".to_string(),
        input: "{}".to_string(),
        provider_executed: false,
        dynamic: false,
        provider_metadata: None,
    };
    let output_call = OutputContent::ToolCall {
        tool_call_id: "call_1".to_string(),
        tool_name: "bash".to_string(),
        input: "{}".to_string(),
        provider_executed: false,
        dynamic: false,
        provider_metadata: None,
    };
    let assistant_call = AssistantPart::tool_call("call_1", "bash", json!({}));

    let stream_json = serde_json::to_value(&stream_call).unwrap();
    let output_json = serde_json::to_value(&output_call).unwrap();
    let assistant_json = serde_json::to_value(&assistant_call).unwrap();

    assert_eq!(type_tag(&stream_json), "tool_call");
    assert_eq!(type_tag(&output_json), "tool_call");
    assert_eq!(type_tag(&assistant_json), "tool_call");

    let stream_input = StreamPart::ToolInputStart {
        id: "call_1".to_string(),
        tool_name: "bash".to_string(),
        provider_executed: false,
        dynamic: false,
        title: None,
        provider_metadata: None,
    };
    assert_eq!(
        type_tag(&serde_json::to_value(&stream_input).unwrap()),
        "tool_input_start"
    );

    let assistant_result = AssistantPart::ToolResult {
        tool_call_id: "call_1".to_string(),
        tool_name: "bash".to_string(),
        output: ToolResultOutput::text("ok"),
        provider_options: None,
    };
    let tool_result = ToolPart::result_text("call_1", "bash", "ok");
    assert_eq!(
        type_tag(&serde_json::to_value(&assistant_result).unwrap()),
        "tool_result"
    );
    assert_eq!(
        type_tag(&serde_json::to_value(&tool_result).unwrap()),
        "tool_result"
    );

    assert_eq!(
        serde_json::to_value(UnifiedFinishReason::ToolCalls).unwrap(),
        json!("tool_calls")
    );
    assert_eq!(
        serde_json::to_value(UnifiedFinishReason::ContentFilter).unwrap(),
        json!("content_filter")
    );
    assert_eq!(
        serde_json::to_value(ReasoningEffort::ProviderDefault).unwrap(),
        json!("provider_default")
    );
    assert_eq!(
        type_tag(&serde_json::to_value(&ToolChoice::specific("bash")).unwrap()),
        "tool"
    );
}

#[test]
fn snake_case_tags_roundtrip_and_reject_kebab_case() {
    let stream_call = StreamPart::ToolCall {
        tool_call_id: "call_1".to_string(),
        tool_name: "bash".to_string(),
        input: "{}".to_string(),
        provider_executed: false,
        dynamic: false,
        provider_metadata: None,
    };
    let encoded = serde_json::to_string(&stream_call).unwrap();
    let decoded: StreamPart = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, stream_call);

    let kebab_call = r#"{"type":"tool-call","tool_call_id":"call_1","tool_name":"bash","input":"{}"}"#;
    assert!(serde_json::from_str::<StreamPart>(kebab_call).is_err());
    assert!(serde_json::from_str::<UnifiedFinishReason>(r#""tool-calls""#).is_err());
    assert!(serde_json::from_str::<ReasoningEffort>(r#""provider-default""#).is_err());
}
