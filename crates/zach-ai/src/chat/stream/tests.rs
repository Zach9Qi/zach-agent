//! 离线验证 Chat Completions SSE 传输、choice 生命周期与收尾顺序。

use super::*;
use futures::StreamExt;
use serde_json::{json, Value};
use zach_ai_core::{GenerateResult, ModelError, StreamAccumulator, UnifiedFinishReason};

/// 一个带固定响应头字段的分块。
pub(super) fn chunk(choices: Vec<Value>) -> Value {
    json!({
        "id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1700000000,
        "model": "example", "choices": choices
    })
}

/// 单 choice 增量分块；`finish` 为 `Some` 时附带结束原因。
pub(super) fn delta(delta: Value, finish: Option<&str>) -> Value {
    chunk(vec![
        json!({"index": 0, "delta": delta, "finish_reason": finish}),
    ])
}

pub(super) fn usage_chunk() -> Value {
    let mut chunk = chunk(vec![]);
    chunk["usage"] = json!({"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5,
        "prompt_tokens_details": {"cached_tokens": 1}});
    chunk
}

pub(super) fn frame(value: &Value) -> String {
    format!("data: {value}\n\n")
}

pub(super) const DONE: &str = "data: [DONE]\n\n";

/// 逐字节交付，确保 UTF-8 字符与帧边界都会跨越传输块。
pub(super) async fn parse_wire(wire: String) -> Vec<Result<StreamPart, ModelError>> {
    let chunks = wire
        .into_bytes()
        .into_iter()
        .map(|b| Ok::<_, std::io::Error>(Bytes::from(vec![b])))
        .collect::<Vec<_>>();
    chat_stream(futures::stream::iter(chunks), ChatStreamParser::new(false))
        .collect()
        .await
}

pub(super) fn aggregate(parts: Vec<Result<StreamPart, ModelError>>) -> GenerateResult {
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    assert!(accumulator.is_complete());
    assert!(
        accumulator.errors().is_empty(),
        "{:?}",
        accumulator.errors()
    );
    accumulator.finish()
}

#[tokio::test]
async fn utf8_text_deltas_and_trailing_usage_produce_one_text_block() {
    let wire = frame(&delta(json!({"role": "assistant", "content": "你"}), None))
        + &frame(&delta(json!({"content": "好"}), None))
        + &frame(&delta(json!({}), Some("stop")))
        + &frame(&usage_chunk())
        + DONE;
    let parts = parse_wire(wire).await;
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::TextStart { .. })))
            .count(),
        1
    );
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::TextEnd { .. })))
            .count(),
        1
    );
    let result = aggregate(parts);
    assert_eq!(result.text(), "你好");
    assert_eq!(result.usage.input_tokens.total, Some(3));
    assert_eq!(result.usage.input_tokens.cache_read, Some(1));
    assert_eq!(result.usage.input_tokens.no_cache, Some(2));
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Stop);
    let response = result.response.unwrap();
    assert_eq!(response.id.as_deref(), Some("chatcmpl-1"));
    assert_eq!(response.timestamp, Some(1700000000000));
}

/// 流式 delta.reasoning_content 需以 Reasoning 事件透出，并在结束前正确闭合。
#[tokio::test]
async fn reasoning_content_deltas_are_exposed_before_text() {
    let wire = frame(&delta(
        json!({"role": "assistant", "reasoning_content": "先"}),
        None,
    )) + &frame(&delta(json!({"reasoning_content": "算"}), None))
        + &frame(&delta(json!({"content": "3"}), None))
        + &frame(&delta(json!({}), Some("stop")))
        + DONE;
    let result = aggregate(parse_wire(wire).await);
    assert_eq!(result.reasoning().as_deref(), Some("先算"));
    assert_eq!(result.text(), "3");
    assert!(matches!(
        result.content[0],
        zach_ai_core::OutputContent::Reasoning { .. }
    ));
}

/// 推理块必须在首个正文或工具增量到达时关闭，而不是拖到流收尾，否则 UI 会在
/// 整段正文输出完毕后才收到"推理结束"。
#[tokio::test]
async fn reasoning_block_closes_when_text_or_tool_calls_begin() {
    let position = |parts: &[Result<StreamPart, ModelError>], pick: fn(&StreamPart) -> bool| {
        parts
            .iter()
            .position(|part| part.as_ref().is_ok_and(pick))
            .unwrap()
    };
    let wire = frame(&delta(json!({"reasoning_content": "想"}), None))
        + &frame(&delta(json!({"content": "答"}), None))
        + &frame(&delta(json!({}), Some("stop")))
        + DONE;
    let parts = parse_wire(wire).await;
    let end = position(&parts, |p| matches!(p, StreamPart::ReasoningEnd { .. }));
    let text = position(&parts, |p| matches!(p, StreamPart::TextDelta { .. }));
    assert!(end < text, "ReasoningEnd 应早于首个 TextDelta");
    aggregate(parts);

    let wire = frame(&delta(json!({"reasoning_content": "想"}), None))
        + &frame(&delta(
            json!({"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "f", "arguments": "{}"}}]}),
            None,
        ))
        + &frame(&delta(json!({}), Some("tool_calls")))
        + DONE;
    let parts = parse_wire(wire).await;
    let end = position(&parts, |p| matches!(p, StreamPart::ReasoningEnd { .. }));
    let tool = position(&parts, |p| matches!(p, StreamPart::ToolInputStart { .. }));
    assert!(end < tool, "ReasoningEnd 应早于 ToolInputStart");
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::ReasoningEnd { .. })))
            .count(),
        1
    );
    aggregate(parts);
}

/// 并行工具调用的收尾事件必须按厂商给出的 index 顺序发出，不能依赖哈希表遍历顺序。
#[tokio::test]
async fn parallel_tool_calls_finish_in_index_order() {
    let calls = |items: Vec<Value>| delta(json!({"tool_calls": items}), None);
    let wire = frame(&calls(vec![
        json!({"index": 0, "id": "call_a", "type": "function", "function": {"name": "add", "arguments": ""}}),
        json!({"index": 1, "id": "call_b", "type": "function", "function": {"name": "sub", "arguments": ""}}),
        json!({"index": 2, "id": "call_c", "type": "function", "function": {"name": "mul", "arguments": ""}}),
    ])) + &frame(&calls(vec![
        json!({"index": 2, "function": {"arguments": "{\"c\":3}"}}),
        json!({"index": 0, "function": {"arguments": "{\"a\":1}"}}),
        json!({"index": 1, "function": {"arguments": "{\"b\":2}"}}),
    ])) + &frame(&delta(json!({}), Some("tool_calls")))
        + DONE;
    let parts = parse_wire(wire).await;
    let order: Vec<&str> = parts
        .iter()
        .filter_map(|part| match part {
            Ok(StreamPart::ToolCall { tool_call_id, .. }) => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["call_a", "call_b", "call_c"]);
    let result = aggregate(parts);
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::ToolCalls);
    assert!(matches!(
        &result.content[1],
        zach_ai_core::OutputContent::ToolCall { tool_call_id, tool_name, input, .. }
            if tool_call_id == "call_b" && tool_name == "sub" && input == "{\"b\":2}"
    ));
}

/// 响应元数据在每个分块里都相同，只透出一次；代理固定附带的 `"error": null` 不是错误。
#[tokio::test]
async fn response_metadata_is_emitted_once_and_null_error_field_is_ignored() {
    let mut with_null_error = delta(json!({"content": "好"}), None);
    with_null_error["error"] = Value::Null;
    let wire = frame(&delta(json!({"role": "assistant", "content": "你"}), None))
        + &frame(&with_null_error)
        + &frame(&delta(json!({}), Some("stop")))
        + &frame(&usage_chunk())
        + DONE;
    let parts = parse_wire(wire).await;
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::ResponseMetadata(_))))
            .count(),
        1
    );
    assert!(!parts
        .iter()
        .any(|p| matches!(p, Ok(StreamPart::Error { .. }))));
    let result = aggregate(parts);
    assert_eq!(result.text(), "你好");
    assert_eq!(result.response.unwrap().id.as_deref(), Some("chatcmpl-1"));
}

/// 服务端在流中下发 `error` 后通常直接断开而不发 `[DONE]`，此时不能再叠加一个
/// 可重试的传输错误，否则上层会把厂商错误误判为传输故障并反复重试。
#[tokio::test]
async fn in_band_error_followed_by_connection_close_is_not_a_transport_error() {
    let wire =
        frame(&json!({"error": {"message": "invalid request", "type": "invalid_request_error"}}));
    let parts = parse_wire(wire).await;
    assert_eq!(parts.len(), 2);
    assert!(matches!(
        &parts[1],
        Ok(StreamPart::Error { message, .. }) if message == "invalid request"
    ));
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    assert_eq!(
        accumulator.finish().finish_reason.unified,
        UnifiedFinishReason::Error
    );
}
