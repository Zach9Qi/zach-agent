//! 截断、损坏分块、流内错误与非法响应不得被解析成成功结果。

use super::*;
use crate::chat::response::parse_response;

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

/// `Finish` 发出之前（没有 finish_reason，或有 finish_reason 但用量与 `[DONE]` 都没到）
/// 断开视为传输错误。
#[tokio::test]
async fn eof_before_finish_is_a_transport_error() {
    for wire in [
        String::new(),
        frame(&delta(json!({"content": "半截"}), None)),
        frame(&delta(json!({"content": "x"}), Some("stop"))),
    ] {
        let parts = parse_wire(wire).await;
        assert!(
            matches!(parts.last(), Some(Err(ModelError::StreamError { .. }))),
            "{parts:?}"
        );
    }
}

/// 用量块到达即发 `Finish`，之后只剩 `[DONE]`；省掉 `[DONE]` 就断开的代理不应让
/// 完整响应被尾部传输错误整体判失败。
#[tokio::test]
async fn connection_loss_after_finish_without_done_keeps_the_response() {
    let wire = frame(&delta(json!({"content": "完整"}), Some("stop"))) + &frame(&usage_chunk());
    let result = aggregate(parse_wire(wire.clone()).await);
    assert_eq!(result.text(), "完整");
    assert_eq!(result.usage.input_tokens.total, Some(3));
    let source = futures::stream::iter(vec![
        Ok(Bytes::from(wire)),
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
    ]);
    let parts = chat_stream(source, ChatStreamParser::new(false), vec![], None)
        .collect::<Vec<_>>()
        .await;
    assert!(parts.iter().all(Result::is_ok));
    assert_eq!(aggregate(parts).text(), "完整");
}

#[tokio::test]
async fn done_without_finish_reason_is_an_error_event_not_a_success() {
    let wire = frame(&delta(json!({"content": "x"}), None)) + DONE;
    let parts = parse_wire(wire).await;
    assert!(parts.iter().all(Result::is_ok));
    assert!(parts
        .iter()
        .any(|p| matches!(p, Ok(StreamPart::Error { .. }))));
    assert!(!parts
        .iter()
        .any(|p| matches!(p, Ok(StreamPart::Finish { .. }))));
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.text(), "x");
}

#[tokio::test]
async fn transport_errors_are_terminal_even_if_more_bytes_follow() {
    let source = futures::stream::iter(vec![
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "断开",
        )),
        Ok(Bytes::from(frame(&delta(json!({}), Some("stop"))) + DONE)),
    ]);
    let parts = chat_stream(source, ChatStreamParser::new(false), vec![], None)
        .collect::<Vec<_>>()
        .await;
    assert_eq!(parts.len(), 2);
    assert!(matches!(
        &parts[1],
        Err(ModelError::StreamError {
            source: Some(_),
            ..
        })
    ));
}

/// 损坏的分块使本轮判为错误，但之后到达的正文与用量仍被收集供诊断。
#[tokio::test]
async fn malformed_chunks_poison_the_turn_but_later_data_is_still_collected() {
    let wire = "data: not json\n\n".to_owned()
        + &frame(&delta(json!({"content": "诊断"}), Some("stop")))
        + &frame(&usage_chunk())
        + DONE;
    let mut accumulator = StreamAccumulator::new();
    for part in parse_wire(wire).await {
        accumulator.process(part.unwrap());
    }
    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.text(), "诊断");
    assert_eq!(result.usage.input_tokens.total, Some(3));
}

#[test]
fn incomplete_or_invalid_json_objects_are_not_successful_responses() {
    for response in [
        json!({}),
        json!({"choices": []}),
        json!({"choices": [{"index": 0}]}),
        json!({"choices": [{"index": 0, "message": {"tool_calls": [{"id": "c"}]}}]}),
    ] {
        assert!(parse_response(response.clone()).is_err(), "{response}");
    }
    // 兼容端点可能省略 finish_reason，按正常结束处理但 raw 为空。
    let lenient = parse_response(json!({
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}}]
    }))
    .unwrap();
    assert_eq!(lenient.finish_reason.unified, UnifiedFinishReason::Stop);
    assert_eq!(lenient.finish_reason.raw, None);
    assert_eq!(lenient.text(), "ok");
    let filtered = parse_response(json!({
        "choices": [{"index": 0, "message": {"role": "assistant", "content": null},
            "finish_reason": "content_filter"}]
    }))
    .unwrap();
    assert_eq!(
        filtered.finish_reason.unified,
        UnifiedFinishReason::ContentFilter
    );
    assert!(filtered.content.is_empty());
}
