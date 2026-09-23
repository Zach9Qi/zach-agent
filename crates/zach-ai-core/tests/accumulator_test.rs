//! 流聚合器顺序、错误与工具结果回放

use serde_json::json;
use zach_ai_core::{
    AssistantPart, FinishReason, Message, OutputContent, ProviderMetadata, ResponseMetadata,
    StreamAccumulator, StreamPart, ToolResultOutput, UnifiedFinishReason, Usage,
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
fn start_and_end_metadata_are_merged_per_provider() {
    let mut start_meta = ProviderMetadata::new();
    start_meta.insert("anthropic", json!({ "item_id": "abc" }));
    let mut end_meta = ProviderMetadata::new();
    end_meta.insert("anthropic", json!({ "signature": "sig" }));
    end_meta.insert("openai", json!({ "cached": true }));

    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::ReasoningStart {
        id: "r1".to_string(),
        provider_metadata: Some(start_meta),
    });
    accumulator.process(StreamPart::ReasoningDelta {
        id: "r1".to_string(),
        delta: "思考".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ReasoningEnd {
        id: "r1".to_string(),
        provider_metadata: Some(end_meta),
    });

    let result = accumulator.finish();
    match &result.content[0] {
        OutputContent::Reasoning {
            provider_metadata, ..
        } => {
            let stored = provider_metadata.as_ref().expect("合并后的元数据");
            let anthropic = stored.get::<serde_json::Value>("anthropic").unwrap();
            assert_eq!(anthropic["item_id"], "abc");
            assert_eq!(anthropic["signature"], "sig");
            assert_eq!(
                stored.get::<serde_json::Value>("openai").unwrap()["cached"],
                true
            );
        }
        other => panic!("期望推理块，实际是 {other:?}"),
    }
}

#[test]
fn response_metadata_is_merged_per_field() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::ResponseMetadata(ResponseMetadata {
        id: Some("resp_1".to_string()),
        timestamp: Some(1),
        model_id: None,
    }));
    accumulator.process(StreamPart::ResponseMetadata(ResponseMetadata {
        id: None,
        timestamp: Some(2),
        model_id: Some("gpt-4o".to_string()),
    }));

    let response = accumulator.finish().response.expect("响应元数据");
    assert_eq!(response.id.as_deref(), Some("resp_1"));
    assert_eq!(response.timestamp, Some(2));
    assert_eq!(response.model_id.as_deref(), Some("gpt-4o"));
}

#[test]
fn empty_reasoning_with_metadata_is_kept_while_bare_empty_block_is_dropped() {
    let mut accumulator = StreamAccumulator::new();
    // 类似 Anthropic redacted_thinking：无正文，信息全在元数据
    accumulator.process(StreamPart::ReasoningStart {
        id: "r1".to_string(),
        provider_metadata: Some(meta()),
    });
    accumulator.process(StreamPart::ReasoningEnd {
        id: "r1".to_string(),
        provider_metadata: None,
    });
    // 纯空块：只 Start 没内容也没元数据
    accumulator.process(StreamPart::TextStart {
        id: "t1".to_string(),
        provider_metadata: None,
    });

    let result = accumulator.finish();
    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        OutputContent::Reasoning {
            text,
            provider_metadata,
        } => {
            assert!(text.is_empty());
            assert!(provider_metadata.is_some());
        }
        other => panic!("期望推理块，实际是 {other:?}"),
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
fn truncated_stream_is_reported_as_unknown() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::TextDelta {
        id: "t1".to_string(),
        delta: "半截".to_string(),
        provider_metadata: None,
    });
    assert!(!accumulator.is_complete());
    assert_eq!(accumulator.finish_reason(), None);

    let result = accumulator.finish();
    assert_eq!(result.text(), "半截");
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Unknown);
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
        message: "overloaded_error".to_string(),
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
    assert_eq!(result.finish_reason.raw.as_deref(), Some("overloaded_error"));
    assert_eq!(result.usage.input_tokens.total, Some(3));
}

#[test]
fn stream_error_without_finish_is_reported_as_error() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::Error {
        message: "overloaded_error".to_string(),
        raw: None,
    });
    assert!(!accumulator.is_complete());

    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.finish_reason.raw.as_deref(), Some("overloaded_error"));
}

#[test]
fn explicit_non_stop_finish_survives_stream_error() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::Error {
        message: "overloaded_error".to_string(),
        raw: None,
    });
    // Error 不终止流：此时尚未收尾，不应提前报出结束原因
    assert!(!accumulator.is_complete());
    assert_eq!(accumulator.finish_reason(), None);

    accumulator.process(StreamPart::Finish {
        usage: Usage::default(),
        finish_reason: FinishReason {
            unified: UnifiedFinishReason::Length,
            raw: Some("length".to_string()),
        },
        provider_metadata: None,
    });
    assert!(accumulator.is_complete());
    assert_eq!(
        accumulator.finish_reason().map(|reason| reason.unified),
        Some(UnifiedFinishReason::Length)
    );

    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Length);
    assert_eq!(result.finish_reason.raw.as_deref(), Some("length"));
}

#[test]
fn all_stream_errors_are_kept_in_order_with_raw() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::Error {
        message: "invalid chunk".to_string(),
        raw: Some(json!("data: {bad")),
    });
    accumulator.process(StreamPart::Error {
        message: String::new(),
        raw: Some(json!({ "type": "overloaded_error" })),
    });

    let errors = accumulator.errors();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].message, "invalid chunk");
    assert_eq!(errors[0].raw, Some(json!("data: {bad")));
    assert_eq!(errors[1].raw, Some(json!({ "type": "overloaded_error" })));

    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.finish_reason.raw.as_deref(), Some("invalid chunk"));
}

#[test]
fn stream_errors_stay_visible_when_finish_is_tool_calls() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::Error {
        message: "invalid chunk".to_string(),
        raw: None,
    });
    accumulator.process(StreamPart::Finish {
        usage: Usage::default(),
        finish_reason: FinishReason::tool_calls(),
        provider_metadata: None,
    });

    assert_eq!(
        accumulator.finish_reason().map(|reason| reason.unified),
        Some(UnifiedFinishReason::ToolCalls)
    );
    assert_eq!(accumulator.errors().len(), 1);
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
    // 同一 call_1 的中间态结果被最终结果原地替换，不残留进度快照
    assert_eq!(result.content.len(), 2);
    match &result.content[0] {
        OutputContent::ToolResult {
            tool_call_id,
            result,
            preliminary,
            ..
        } => {
            assert_eq!(tool_call_id, "call_1");
            assert_eq!(result, &json!({ "hits": 1 }));
            assert!(!preliminary);
        }
        other => panic!("期望工具结果，实际是 {other:?}"),
    }

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
