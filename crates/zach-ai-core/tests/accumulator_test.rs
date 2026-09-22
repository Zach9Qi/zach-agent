//! 流聚合器顺序、错误与工具结果回放

use serde_json::json;
use zach_ai_core::{
    AssistantPart, FinishReason, Message, OutputContent, ProviderMetadata, StreamAccumulator,
    StreamPart, ToolResultOutput, UnifiedFinishReason, Usage,
};

fn meta() -> ProviderMetadata {
    let mut metadata = ProviderMetadata::new();
    metadata.insert("openai", json!({ "cached": true }));
    metadata
}

#[test]
fn unclosed_parts_keep_first_seen_order() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::TextDelta {
        id: "t1".to_string(),
        delta: "你好".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolCall {
        tool_call_id: "call_1".to_string(),
        tool_name: "calculator".to_string(),
        input: "{\"expr\":\"1+1\"}".to_string(),
        provider_executed: false,
        dynamic: false,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextDelta {
        id: "t2".to_string(),
        delta: "世界".to_string(),
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.content.len(), 3);
    assert_eq!(result.content[0].as_text(), Some("你好"));
    match &result.content[1] {
        OutputContent::ToolCall {
            tool_call_id,
            tool_name,
            input,
            ..
        } => {
            assert_eq!(tool_call_id, "call_1");
            assert_eq!(tool_name, "calculator");
            assert_eq!(input, "{\"expr\":\"1+1\"}");
        }
        other => panic!("期望工具调用，实际是 {other:?}"),
    }
    assert_eq!(result.content[2].as_text(), Some("世界"));
}

#[test]
fn text_start_does_not_wipe_earlier_deltas() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::TextDelta {
        id: "t1".to_string(),
        delta: "你好".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextStart {
        id: "t1".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextDelta {
        id: "t1".to_string(),
        delta: "世界".to_string(),
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.text(), "你好世界");
}

#[test]
fn tool_delta_before_start_is_kept() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::ToolInputDelta {
        id: "call_1".to_string(),
        delta: "{\"a\":".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputStart {
        id: "call_1".to_string(),
        tool_name: "calculator".to_string(),
        provider_executed: false,
        dynamic: false,
        title: None,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputDelta {
        id: "call_1".to_string(),
        delta: "1}".to_string(),
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        OutputContent::ToolCall {
            tool_name, input, ..
        } => {
            assert_eq!(tool_name, "calculator");
            assert_eq!(input, "{\"a\":1}");
        }
        other => panic!("期望工具调用，实际是 {other:?}"),
    }
}

#[test]
fn end_event_metadata_is_preserved() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::TextStart {
        id: "t1".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextDelta {
        id: "t1".to_string(),
        delta: "你好".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextEnd {
        id: "t1".to_string(),
        provider_metadata: Some(meta()),
    });
    accumulator.process(StreamPart::ToolInputStart {
        id: "call_1".to_string(),
        tool_name: "calculator".to_string(),
        provider_executed: false,
        dynamic: false,
        title: None,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputEnd {
        id: "call_1".to_string(),
        provider_metadata: Some(meta()),
    });

    let result = accumulator.finish();
    match &result.content[0] {
        OutputContent::Text {
            text,
            provider_metadata,
        } => {
            assert_eq!(text, "你好");
            let stored = provider_metadata.as_ref().expect("文本结束元数据");
            assert_eq!(
                stored.get::<serde_json::Value>("openai").unwrap()["cached"],
                true
            );
        }
        other => panic!("期望文本，实际是 {other:?}"),
    }
    match &result.content[1] {
        OutputContent::ToolCall {
            provider_metadata, ..
        } => {
            let stored = provider_metadata.as_ref().expect("工具结束元数据");
            assert_eq!(
                stored.get::<serde_json::Value>("openai").unwrap()["cached"],
                true
            );
        }
        other => panic!("期望工具调用，实际是 {other:?}"),
    }
}

#[test]
fn interleaved_text_stays_before_later_tool_call() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::TextStart {
        id: "t1".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolCall {
        tool_call_id: "call_1".to_string(),
        tool_name: "calculator".to_string(),
        input: "{}".to_string(),
        provider_executed: false,
        dynamic: false,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextDelta {
        id: "t1".to_string(),
        delta: "你好".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextEnd {
        id: "t1".to_string(),
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.content[0].as_text(), Some("你好"));
    assert!(matches!(result.content[1], OutputContent::ToolCall { .. }));
}

#[test]
fn stream_error_is_not_reported_as_stop() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::TextDelta {
        id: "t1".to_string(),
        delta: "部分".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::Error {
        message: "连接中断".to_string(),
        raw: None,
    });
    accumulator.process(StreamPart::Finish {
        usage: Usage::simple(3, 1),
        finish_reason: FinishReason::stop(),
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.text(), "部分");
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.finish_reason.raw.as_deref(), Some("连接中断"));
    assert_eq!(result.usage.input_tokens.total, Some(3));
}

#[test]
fn explicit_non_stop_finish_survives_stream_error() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::Error {
        message: "连接中断".to_string(),
        raw: None,
    });
    accumulator.process(StreamPart::Finish {
        usage: Usage::default(),
        finish_reason: FinishReason {
            unified: UnifiedFinishReason::Length,
            raw: Some("length".to_string()),
        },
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Length);
    assert_eq!(result.finish_reason.raw.as_deref(), Some("length"));
}

#[test]
fn streamed_and_complete_tool_call_collapse_to_one() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::ToolInputStart {
        id: "call_1".to_string(),
        tool_name: "calculator".to_string(),
        provider_executed: false,
        dynamic: false,
        title: None,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputDelta {
        id: "call_1".to_string(),
        delta: "{\"expr\":".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolCall {
        tool_call_id: "call_1".to_string(),
        tool_name: "calculator".to_string(),
        input: "{\"expr\":\"1+1\"}".to_string(),
        provider_executed: true,
        dynamic: true,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputDelta {
        id: "call_1".to_string(),
        delta: "ignored".to_string(),
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        OutputContent::ToolCall {
            input,
            provider_executed,
            dynamic,
            ..
        } => {
            assert_eq!(input, "{\"expr\":\"1+1\"}");
            assert!(*provider_executed);
            assert!(*dynamic);
        }
        other => panic!("期望工具调用，实际是 {other:?}"),
    }
}

#[test]
fn empty_tool_input_replays_as_empty_object() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::ToolInputStart {
        id: "call_1".to_string(),
        tool_name: "now".to_string(),
        provider_executed: false,
        dynamic: false,
        title: None,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputEnd {
        id: "call_1".to_string(),
        provider_metadata: None,
    });

    match accumulator.finish().into_assistant_message() {
        Message::Assistant { content, .. } => match &content[0] {
            AssistantPart::ToolCall { input, .. } => assert_eq!(input, &json!({})),
            other => panic!("期望工具调用，实际是 {other:?}"),
        },
        other => panic!("期望助手消息，实际是 {other:?}"),
    }
}

#[test]
fn provider_tool_result_is_replayed_into_assistant_message() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::ToolResult {
        tool_call_id: "call_1".to_string(),
        tool_name: "web_search".to_string(),
        result: json!("临时结果"),
        is_error: false,
        preliminary: true,
        dynamic: false,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolResult {
        tool_call_id: "call_1".to_string(),
        tool_name: "web_search".to_string(),
        result: json!({ "hits": 1 }),
        is_error: false,
        preliminary: false,
        dynamic: false,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolResult {
        tool_call_id: "call_2".to_string(),
        tool_name: "web_search".to_string(),
        result: json!("超时"),
        is_error: true,
        preliminary: false,
        dynamic: false,
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.content.len(), 3);

    match result.into_assistant_message() {
        Message::Assistant { content, .. } => {
            assert_eq!(content.len(), 2);
            match &content[0] {
                AssistantPart::ToolResult {
                    tool_call_id,
                    tool_name,
                    output,
                    ..
                } => {
                    assert_eq!(tool_call_id, "call_1");
                    assert_eq!(tool_name, "web_search");
                    assert_eq!(output, &ToolResultOutput::json(json!({ "hits": 1 })));
                }
                other => panic!("期望 JSON 工具结果，实际是 {other:?}"),
            }
            match &content[1] {
                AssistantPart::ToolResult { output, .. } => {
                    assert_eq!(output, &ToolResultOutput::error_text("超时"));
                }
                other => panic!("期望错误文本，实际是 {other:?}"),
            }
        }
        other => panic!("期望助手消息，实际是 {other:?}"),
    }
}
