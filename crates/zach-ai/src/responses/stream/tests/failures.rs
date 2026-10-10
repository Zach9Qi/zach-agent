//! 截断、损坏事件与失败状态不得被解析成成功结果。

use super::*;
use crate::responses::response::parse_response;
use zach_ai_core::UnifiedFinishReason;

#[tokio::test]
async fn eof_without_terminal_event_is_a_transport_error() {
    for wire in [
        String::new(),
        "data: [DONE]\n\n".into(),
        "data: {\"type\":\"response.completed\"}".into(),
        frame(
            &json!({"type": "response.output_text.delta", "item_id": "msg_1", "content_index": 0, "delta": "半截"}),
        ),
    ] {
        let parts = parse_wire(wire).await;
        assert!(matches!(
            parts.last(),
            Some(Err(ModelError::StreamError { .. }))
        ));
        assert!(!parts
            .iter()
            .any(|p| matches!(p, Ok(StreamPart::Finish { .. }))));
    }
}

#[tokio::test]
async fn transport_errors_are_terminal_even_if_more_bytes_follow() {
    let source = futures::stream::iter(vec![
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "断开",
        )),
        Ok(Bytes::from(frame(
            &json!({"type": "response.completed", "response": response(vec![])}),
        ))),
    ]);
    let parts = responses_stream(source, ResponsesStreamParser::new(false), vec![])
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

#[tokio::test]
async fn malformed_events_cannot_be_rescued_by_a_later_completed_response() {
    let wire = "data: invalid\n\n".to_owned()
        + &frame(
            &json!({"type": "response.completed", "response": response(vec![message("诊断")])}),
        );
    let mut accumulator = StreamAccumulator::new();
    for part in parse_wire(wire).await {
        accumulator.process(part.unwrap());
    }
    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.usage.input_tokens.total, Some(100));
}

/// 终止事件中出现未适配的输出项时，Finish（用量与停止原因）不能被整体吞掉，
/// 其余输出项也要继续解析；非流式路径保持整体报错语义不变。
#[tokio::test]
async fn unadapted_output_item_cannot_swallow_finish_and_usage() {
    let unknown = json!({"type": "web_search_call", "id": "ws_1", "status": "completed"});
    let payload = response(vec![unknown, message("答案")]);
    let wire = frame(&json!({"type": "response.completed", "response": payload}));
    let parts = parse_wire(wire).await;
    assert!(parts.iter().all(Result::is_ok));
    assert!(parts
        .iter()
        .any(|p| matches!(p, Ok(StreamPart::Error { .. }))));
    assert!(parts.iter().any(|p| matches!(
        p,
        Ok(StreamPart::Finish { usage, .. }) if usage.input_tokens.total == Some(100)
    )));
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    let result = accumulator.finish();
    assert_eq!(result.text(), "答案");
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert!(parse_response(payload).is_err());
}

#[tokio::test]
async fn failed_and_incomplete_responses_have_distinct_outcomes() {
    let mut failed = response(vec![]);
    failed["status"] = json!("failed");
    failed["error"] = json!({"message": "生成失败"});
    assert!(parse_response(failed.clone()).is_err());
    let wire = frame(&json!({"type": "response.failed", "response": failed}));
    let parts = parse_wire(wire).await;
    assert!(parts
        .iter()
        .any(|p| matches!(p, Ok(StreamPart::Error { message, .. }) if message == "生成失败")));
    let mut incomplete = response(vec![message("半截")]);
    incomplete["status"] = json!("incomplete");
    incomplete["incomplete_details"] = json!({"reason": "max_output_tokens"});
    let result = parse_response(incomplete.clone()).unwrap();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Length);
    let wire = frame(&json!({"type": "response.incomplete", "response": incomplete}));
    assert_eq!(aggregate(parse_wire(wire).await), result);
    incomplete["incomplete_details"]["reason"] = json!("content_filter");
    assert_eq!(
        parse_response(incomplete).unwrap().finish_reason.unified,
        UnifiedFinishReason::ContentFilter
    );
}

/// 厂商新增的 `incomplete_details.reason` 不能让整轮变成错误：`Finish`（含用量）必须照常
/// 发出，原始原因保留在 `raw` 与元数据中。此前流式路径会因此丢失终止事件。
#[tokio::test]
async fn unknown_incomplete_reason_keeps_finish_and_usage() {
    let mut incomplete = response(vec![message("半截")]);
    incomplete["status"] = json!("incomplete");
    incomplete["incomplete_details"] = json!({"reason": "brand_new_reason"});
    let generated = parse_response(incomplete.clone()).unwrap();
    assert_eq!(generated.finish_reason.unified, UnifiedFinishReason::Other);
    assert_eq!(
        generated.finish_reason.raw.as_deref(),
        Some("brand_new_reason")
    );
    assert_eq!(generated.usage.input_tokens.total, Some(100));
    let wire = frame(&json!({"type": "response.incomplete", "response": incomplete}));
    let parts = parse_wire(wire).await;
    assert!(parts.iter().all(Result::is_ok));
    assert_eq!(aggregate(parts), generated);
    let openai = generated
        .provider_metadata
        .unwrap()
        .get::<Value>("openai")
        .unwrap();
    assert_eq!(openai["incomplete_details"]["reason"], "brand_new_reason");
}

#[test]
fn incomplete_or_invalid_json_objects_are_not_successful_responses() {
    for response in [
        json!({}),
        json!({"status": "completed"}),
        json!({"status": "in_progress", "output": []}),
    ] {
        assert!(parse_response(response).is_err());
    }
    let mut parser = ResponsesStreamParser::new(false);
    let parts = parser.data(
        &json!({"type": "response.function_call_arguments.delta",
        "item_id": "missing", "delta": "{}"})
        .to_string(),
    );
    assert!(matches!(&parts[0], StreamPart::Error { .. }));
}
