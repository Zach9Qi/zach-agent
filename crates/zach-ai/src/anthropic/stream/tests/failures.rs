//! 截断、损坏事件、流内错误与非法响应不得被解析成成功结果。

use super::*;
use crate::anthropic::response::parse_response;
use zach_ai_core::UnifiedFinishReason;

#[tokio::test]
async fn eof_before_finish_is_a_transport_error() {
    for wire in [
        String::new(),
        start_frame(),
        start_frame()
            + &event_frame(&json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": "半截"}})),
    ] {
        let parts = parse_wire(wire).await;
        assert!(matches!(
            parts.last(),
            Some(Err(ModelError::StreamError { .. }))
        ));
    }
}

/// `message_delta` 带 `stop_reason` 后响应已完整，`message_stop` 之前断开
/// （FIN 或 RST）不能把一次成功的响应整体判为失败。
#[tokio::test]
async fn connection_loss_after_finish_without_message_stop_keeps_the_response() {
    let wire = start_frame()
        + &event_frame(&json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": "完整"}}))
        + &event_frame(&json!({"type": "content_block_stop", "index": 0}))
        + &event_frame(
            &json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 1}}),
        );
    let result = aggregate(parse_wire(wire.clone()).await);
    assert_eq!(result.text(), "完整");
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Stop);
    let source = futures::stream::iter(vec![
        Ok(Bytes::from(wire)),
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
    ]);
    let parts = messages_stream(source, MessagesStreamParser::new(false), vec![], None)
        .collect::<Vec<_>>()
        .await;
    assert!(parts.iter().all(Result::is_ok));
    assert_eq!(aggregate(parts).text(), "完整");
}

#[tokio::test]
async fn message_stop_without_stop_reason_is_an_error_event() {
    let wire = start_frame() + &event_frame(&json!({"type": "message_stop"}));
    let parts = parse_wire(wire).await;
    assert!(parts
        .iter()
        .any(|p| matches!(p, Ok(StreamPart::Error { .. }))));
    assert!(!parts.iter().any(|p| p.is_err()));
    assert_eq!(
        aggregate_lenient(parts).finish_reason.unified,
        UnifiedFinishReason::Error
    );
}

#[tokio::test]
async fn malformed_events_and_error_events_poison_the_turn() {
    let wire = "data: invalid\n\n".to_owned()
        + &start_frame()
        + &event_frame(&json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": "诊断"}}))
        + &event_frame(&json!({"type": "content_block_stop", "index": 0}))
        + &stop_frames("end_turn");
    let result = aggregate_lenient(parse_wire(wire).await);
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.text(), "诊断");
    assert_eq!(result.usage.input_tokens.total, Some(100));

    let wire = start_frame()
        + &event_frame(
            &json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
        );
    let parts = parse_wire(wire).await;
    assert!(parts
        .iter()
        .any(|p| matches!(p, Ok(StreamPart::Error { message, .. }) if message == "Overloaded")));
    // 服务端已报告错误，连接随后关闭不再额外视为传输故障。
    assert!(!parts.iter().any(|p| p.is_err()));
}

#[test]
fn mismatched_or_duplicate_block_events_are_rejected() {
    let mut parser = MessagesStreamParser::new(false);
    let start = json!({"type": "content_block_start", "index": 0,
        "content_block": {"type": "text", "text": ""}});
    assert!(!parser
        .data(&start.to_string())
        .iter()
        .any(|p| matches!(p, StreamPart::Error { .. })));
    for event in [
        start,
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{}"}}),
        json!({"type": "content_block_delta", "index": 9,
            "delta": {"type": "text_delta", "text": "x"}}),
        json!({"type": "content_block_stop", "index": 9}),
    ] {
        let parts = parser.data(&event.to_string());
        assert!(
            matches!(parts.last(), Some(StreamPart::Error { .. })),
            "{event}"
        );
    }
    let mut parser = MessagesStreamParser::new(false);
    parser.data(
        &json!({"type": "content_block_start", "index": 0,
        "content_block": {"type": "text", "text": ""}})
        .to_string(),
    );
    parser.data(&json!({"type": "content_block_stop", "index": 0}).to_string());
    let parts = parser.data(
        &json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": "迟到"}})
        .to_string(),
    );
    assert!(matches!(parts.last(), Some(StreamPart::Error { .. })));
}

#[test]
fn incomplete_or_invalid_json_objects_are_not_successful_responses() {
    for response in [
        json!({}),
        json!({"content": []}),
        json!({"content": [{"type": "text", "text": "x"}], "stop_reason": null}),
        json!({"type": "error", "error": {"type": "invalid_request_error", "message": "bad"}}),
    ] {
        assert!(parse_response(response).is_err());
    }
    let mut refusal = message(vec![], "refusal");
    refusal["usage"] = json!({"input_tokens": 1, "output_tokens": 0});
    let result = parse_response(refusal).unwrap();
    assert_eq!(
        result.finish_reason.unified,
        UnifiedFinishReason::ContentFilter
    );
    assert_eq!(result.usage.input_tokens.total, Some(1));
    assert_eq!(result.usage.input_tokens.cache_read, None);
    let truncated = parse_response(message(vec![], "max_tokens")).unwrap();
    assert_eq!(truncated.finish_reason.unified, UnifiedFinishReason::Length);
    let paused = parse_response(message(vec![], "pause_turn")).unwrap();
    assert_eq!(paused.finish_reason.unified, UnifiedFinishReason::Other);
    assert_eq!(paused.finish_reason.raw.as_deref(), Some("pause_turn"));
}

/// 安全分类器拒绝时内容为空，`stop_details` 是唯一的诊断信息，流式与非流式都必须透出。
#[tokio::test]
async fn refusal_details_are_exposed_in_finish_metadata() {
    let details =
        json!({"type": "refusal", "category": "reasoning_extraction", "explanation": "blocked"});
    let mut refusal = message(vec![], "refusal");
    refusal["stop_details"] = details.clone();
    let generated = parse_response(refusal).unwrap();
    let wire = start_frame()
        + &event_frame(&json!({"type": "message_delta",
            "delta": {"stop_reason": "refusal", "stop_sequence": null, "stop_details": details},
            "usage": {"output_tokens": 20}}))
        + &event_frame(&json!({"type": "message_stop"}));
    let streamed = aggregate(parse_wire(wire).await);
    for result in [generated, streamed] {
        assert_eq!(
            result.finish_reason.unified,
            UnifiedFinishReason::ContentFilter
        );
        assert!(result.content.is_empty());
        let anthropic = result
            .provider_metadata
            .unwrap()
            .get::<Value>("anthropic")
            .unwrap();
        assert_eq!(
            anthropic["stop_details"]["category"],
            "reasoning_extraction"
        );
    }
}
