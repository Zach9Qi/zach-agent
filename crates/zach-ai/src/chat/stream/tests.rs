//! 离线验证 Chat Completions SSE 传输与 choice 生命周期；失败路径与回放见子模块。

mod failures;
mod replay;

use super::*;
use crate::test_support::{aggregate, aggregate_lenient, byte_stream, data_frame};
use futures::StreamExt;
use serde_json::{json, Value};
use zach_ai_core::{ModelError, UnifiedFinishReason};

/// 一个带固定响应头字段的分块。
fn chunk(choices: Vec<Value>) -> Value {
    json!({
        "id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1700000000,
        "model": "example", "choices": choices
    })
}

/// 单 choice 增量分块；`finish` 为 `Some` 时附带结束原因。
fn delta(delta: Value, finish: Option<&str>) -> Value {
    chunk(vec![
        json!({"index": 0, "delta": delta, "finish_reason": finish}),
    ])
}

fn usage_chunk() -> Value {
    let mut chunk = chunk(vec![]);
    chunk["usage"] = json!({"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5,
        "prompt_tokens_details": {"cached_tokens": 1}});
    chunk
}

const DONE: &str = "data: [DONE]\n\n";

/// 事件在序列中的位置，用于断言先后顺序。
fn position(parts: &[Result<StreamPart, ModelError>], pick: fn(&StreamPart) -> bool) -> usize {
    parts
        .iter()
        .position(|part| part.as_ref().is_ok_and(pick))
        .unwrap()
}

async fn parse_wire(wire: String) -> Vec<Result<StreamPart, ModelError>> {
    chat_stream(
        byte_stream(wire),
        ChatStreamParser::new(false),
        vec![],
        None,
    )
    .collect()
    .await
}

#[tokio::test]
async fn utf8_text_deltas_and_trailing_usage_produce_one_text_block() {
    let wire = data_frame(&delta(json!({"role": "assistant", "content": "你"}), None))
        + &data_frame(&delta(json!({"content": "好"}), None))
        + &data_frame(&delta(json!({}), Some("stop")))
        + &data_frame(&usage_chunk())
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

/// OpenRouter、Ollama、Groq 等端点用 `reasoning` 字段下发思考链，与 `reasoning_content` 同等对待；
/// 结构化取值（如推理明细数组）不是正文，忽略。
#[tokio::test]
async fn reasoning_field_is_treated_like_reasoning_content() {
    let wire = data_frame(&delta(
        json!({"role": "assistant", "reasoning": "先", "reasoning_details": [{"type": "x"}]}),
        None,
    )) + &data_frame(&delta(json!({"reasoning": "算"}), None))
        + &data_frame(&delta(json!({"content": "3"}), None))
        + &data_frame(&delta(json!({}), Some("stop")))
        + DONE;
    let result = aggregate(parse_wire(wire).await);
    assert_eq!(result.reasoning().as_deref(), Some("先算"));
    assert_eq!(result.text(), "3");
}

/// 推理块必须在首个正文或工具增量到达时关闭，而不是拖到流收尾，否则 UI 会在
/// 整段正文输出完毕后才收到"推理结束"。
#[tokio::test]
async fn reasoning_block_closes_when_text_or_tool_calls_begin() {
    let wire = data_frame(&delta(json!({"reasoning_content": "想"}), None))
        + &data_frame(&delta(json!({"content": "答"}), None))
        + &data_frame(&delta(json!({}), Some("stop")))
        + DONE;
    let parts = parse_wire(wire).await;
    let end = position(&parts, |p| matches!(p, StreamPart::ReasoningEnd { .. }));
    let text = position(&parts, |p| matches!(p, StreamPart::TextDelta { .. }));
    assert!(end < text, "ReasoningEnd 应早于首个 TextDelta");
    aggregate(parts);

    let wire = data_frame(&delta(json!({"reasoning_content": "想"}), None))
        + &data_frame(&delta(
            json!({"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "f", "arguments": "{}"}}]}),
            None,
        ))
        + &data_frame(&delta(json!({}), Some("tool_calls")))
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

/// LiteLLM、vLLM 等网关在思考阶段伴发 `content: ""` 占位：不能据此提前结束推理块，
/// 也不能为占位开出空文本块（空块会带着元数据留在最终内容里）。
#[tokio::test]
async fn empty_content_placeholders_do_not_end_reasoning_or_open_text() {
    let wire = data_frame(&delta(
        json!({"role": "assistant", "reasoning_content": "想", "content": ""}),
        None,
    )) + &data_frame(&delta(
        json!({"reasoning_content": "着", "content": ""}),
        None,
    )) + &data_frame(&delta(json!({"content": "答"}), None))
        + &data_frame(&delta(json!({}), Some("stop")))
        + DONE;
    let parts = parse_wire(wire).await;
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::ReasoningDelta { .. })))
            .count(),
        2
    );
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::ReasoningEnd { .. })))
            .count(),
        1
    );
    let end = position(&parts, |p| matches!(p, StreamPart::ReasoningEnd { .. }));
    let text_start = position(&parts, |p| matches!(p, StreamPart::TextStart { .. }));
    assert!(end < text_start, "推理块应持续到首个非空正文");
    let result = aggregate(parts);
    assert_eq!(result.reasoning().as_deref(), Some("想着"));
    assert_eq!(result.text(), "答");
    assert_eq!(
        result.content.len(),
        2,
        "不应残留空文本块: {:?}",
        result.content
    );
}

/// `Finish` 发出后本轮已收尾，迟到的增量分块（含正文、推理与新 choice）不再产生任何事件。
#[tokio::test]
async fn late_chunks_after_finish_emit_nothing() {
    let late = chunk(vec![
        json!({"index": 0, "delta": {"content": "迟到", "reasoning_content": "迟"},
            "finish_reason": null}),
        json!({"index": 1, "delta": {"role": "assistant", "content": "新块"},
            "finish_reason": null}),
    ]);
    let wire = data_frame(&delta(json!({"content": "x"}), Some("stop")))
        + &data_frame(&usage_chunk())
        + &data_frame(&late)
        + DONE;
    let parts = parse_wire(wire).await;
    assert!(
        matches!(parts.last(), Some(Ok(StreamPart::Finish { .. }))),
        "{parts:?}"
    );
    assert_eq!(aggregate(parts).text(), "x");
}

/// 响应元数据在每个分块里都相同，只透出一次；代理固定附带的 `"error": null` 不是错误。
#[tokio::test]
async fn response_metadata_is_emitted_once_and_null_error_field_is_ignored() {
    let mut with_null_error = delta(json!({"content": "好"}), None);
    with_null_error["error"] = Value::Null;
    let wire = data_frame(&delta(json!({"role": "assistant", "content": "你"}), None))
        + &data_frame(&with_null_error)
        + &data_frame(&delta(json!({}), Some("stop")))
        + &data_frame(&usage_chunk())
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

/// 收尾事件（含 `Finish`）在用量分块到达时就发出，不必等待 `[DONE]`；
/// 之后的 `[DONE]` 只是终止读取，不会重复收尾。
#[tokio::test]
async fn finish_is_emitted_with_usage_and_not_repeated_at_done() {
    let wire = data_frame(&delta(json!({"content": "x"}), Some("stop")))
        + &data_frame(&usage_chunk())
        + DONE;
    let parts = parse_wire(wire).await;
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::Finish { .. })))
            .count(),
        1
    );
    let finish = position(&parts, |p| matches!(p, StreamPart::Finish { .. }));
    assert_eq!(finish, parts.len() - 1);
    assert_eq!(aggregate(parts).usage.output_tokens.total, Some(2));
}
