//! 工具入参解析：严格解析、回放归一、展示保留原文，以及回放边界的契约

use serde_json::{json, Value};
use zach_ai_core::tool::{parse_tool_input, tool_input_for_display, tool_input_for_replay};
use zach_ai_core::{AssistantPart, OutputContent};

/// 无法接受的入参：非法 JSON、截断、以及合法但非对象的值
const REJECTED: &[&str] = &[
    "{bad", r#"{"a":"#, "[]", "[1,2]", "42", "null", "true", r#""s""#,
];

#[test]
fn blank_input_parses_to_empty_object() {
    for raw in ["", "   ", "\n\t"] {
        assert_eq!(parse_tool_input(raw).unwrap(), json!({}), "输入: {raw:?}");
    }
}

#[test]
fn valid_object_parses_as_is() {
    let value = parse_tool_input(r#" {"a": 1, "b": [true]} "#).unwrap();
    assert_eq!(value, json!({"a": 1, "b": [true]}));
}

#[test]
fn malformed_or_non_object_input_is_rejected() {
    for raw in REJECTED {
        let error = parse_tool_input(raw).expect_err(&format!("应拒绝: {raw:?}"));
        assert!(
            error.to_string().contains("JSON 对象"),
            "错误文案应说明要求 JSON 对象: {error}"
        );
    }
}

#[test]
fn replay_normalizes_rejected_input_to_empty_object() {
    for raw in REJECTED {
        assert_eq!(tool_input_for_replay(raw), json!({}), "输入: {raw:?}");
    }
    assert_eq!(tool_input_for_replay(r#"{"a":1}"#), json!({"a": 1}));
}

#[test]
fn display_keeps_raw_text_for_rejected_input() {
    for raw in REJECTED {
        assert_eq!(
            tool_input_for_display(raw),
            Value::String(raw.to_string()),
            "输入: {raw:?}"
        );
    }
    assert_eq!(tool_input_for_display(r#"{"a":1}"#), json!({"a": 1}));
}

/// 回放边界的契约：`AssistantPart::ToolCall.input` 永远是 JSON Object
#[test]
fn tool_call_replayed_into_history_is_always_an_object() {
    for raw in REJECTED.iter().copied().chain(["", r#"{"a":1}"#]) {
        let part = OutputContent::ToolCall {
            tool_call_id: "c1".into(),
            tool_name: "echo".into(),
            input: raw.to_string(),
            provider_executed: false,
            dynamic: false,
            provider_metadata: None,
        }
        .into_assistant_part()
        .expect("工具调用应进入历史");
        match part {
            AssistantPart::ToolCall { input, .. } => {
                assert!(
                    input.is_object(),
                    "输入 {raw:?} 回放后应为对象，实际: {input}"
                );
            }
            other => panic!("应为 ToolCall，实际: {other:?}"),
        }
    }
}
