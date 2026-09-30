//! 流聚合器的错误优先级、诊断信息与收尾状态

use serde_json::json;
use zach_ai_core::{
    FinishReason, ProviderMetadata, StreamAccumulator, StreamPart, UnifiedFinishReason, Usage,
};

fn error() -> StreamPart {
    StreamPart::Error {
        message: "分块解析失败".into(),
        raw: Some(json!("data: {bad")),
    }
}

fn finish(unified: UnifiedFinishReason) -> StreamPart {
    StreamPart::Finish {
        usage: Usage::default(),
        finish_reason: FinishReason {
            unified,
            raw: Some("厂商结束原因".into()),
        },
        provider_metadata: None,
    }
}

fn reasons() -> [UnifiedFinishReason; 7] {
    [
        UnifiedFinishReason::Stop,
        UnifiedFinishReason::Length,
        UnifiedFinishReason::ContentFilter,
        UnifiedFinishReason::ToolCalls,
        UnifiedFinishReason::Error,
        UnifiedFinishReason::Other,
        UnifiedFinishReason::Unknown,
    ]
}

#[test]
fn stream_error_overrides_every_finish_reason_in_either_order() {
    for reason in reasons() {
        for error_first in [true, false] {
            let mut accumulator = StreamAccumulator::new();
            let parts = if error_first {
                [error(), finish(reason)]
            } else {
                [finish(reason), error()]
            };
            for part in parts {
                accumulator.process(part);
            }

            assert!(accumulator.is_complete());
            let expected = FinishReason {
                unified: UnifiedFinishReason::Error,
                raw: Some("分块解析失败".into()),
            };
            assert_eq!(accumulator.finish_reason(), Some(expected.clone()));
            assert_eq!(accumulator.errors().len(), 1);
            assert_eq!(accumulator.finish().finish_reason, expected);
        }
    }
}

#[test]
fn finish_reason_is_preserved_when_no_stream_error_occurs() {
    for reason in reasons() {
        let mut accumulator = StreamAccumulator::new();
        accumulator.process(finish(reason));
        let expected = FinishReason {
            unified: reason,
            raw: Some("厂商结束原因".into()),
        };
        assert!(accumulator.errors().is_empty());
        assert_eq!(accumulator.finish_reason(), Some(expected.clone()));
        assert_eq!(accumulator.finish().finish_reason, expected);
    }
}

#[test]
fn later_finishes_cannot_restore_success_after_an_error() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(error());
    for reason in reasons() {
        accumulator.process(finish(reason));
        assert_eq!(
            accumulator.finish_reason().unwrap().unified,
            UnifiedFinishReason::Error
        );
    }
    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.finish_reason.raw.as_deref(), Some("分块解析失败"));
}

#[test]
fn stream_error_without_finish_is_reported_as_error() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(error());
    // 错误决定最终失败，但不等同于已收到收尾事件。
    assert!(!accumulator.is_complete());
    assert_eq!(accumulator.finish_reason(), None);
    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.finish_reason.raw.as_deref(), Some("分块解析失败"));
}

#[test]
fn truncated_stream_is_reported_as_unknown() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::TextDelta {
        id: "t1".into(),
        delta: "半截".into(),
        provider_metadata: None,
    });
    assert!(!accumulator.is_complete());
    assert_eq!(accumulator.finish_reason(), None);

    let result = accumulator.finish();
    assert_eq!(result.text(), "半截");
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Unknown);
}

#[test]
fn errors_do_not_prevent_collecting_content_usage_and_metadata() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(error());
    accumulator.process(StreamPart::TextDelta {
        id: "t1".into(),
        delta: "诊断用的部分内容".into(),
        provider_metadata: None,
    });
    let mut metadata = ProviderMetadata::new();
    metadata.insert("openai", json!({ "service_tier": "default" }));
    accumulator.process(StreamPart::Finish {
        usage: Usage::simple(3, 1),
        finish_reason: FinishReason::tool_calls(),
        provider_metadata: Some(metadata.clone()),
    });

    let result = accumulator.finish();
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
    assert_eq!(result.text(), "诊断用的部分内容");
    assert_eq!(result.usage, Usage::simple(3, 1));
    assert_eq!(result.provider_metadata, Some(metadata));
}

#[test]
fn all_stream_errors_are_kept_in_order_and_first_error_is_the_cause() {
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(error());
    accumulator.process(StreamPart::Error {
        message: "后续错误".into(),
        raw: Some(json!({ "type": "overloaded_error" })),
    });
    accumulator.process(finish(UnifiedFinishReason::ToolCalls));

    let errors = accumulator.errors();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].message, "分块解析失败");
    assert_eq!(errors[0].raw, Some(json!("data: {bad")));
    assert_eq!(errors[1].message, "后续错误");
    assert_eq!(errors[1].raw, Some(json!({ "type": "overloaded_error" })));
    assert_eq!(
        accumulator.finish().finish_reason.raw.as_deref(),
        Some("分块解析失败")
    );
}

#[test]
fn empty_error_message_falls_back_to_raw_or_default_description() {
    let raw = json!({ "type": "overloaded_error" });
    for (payload, expected) in [
        (Some(raw.clone()), raw.to_string()),
        (None, "未知流式错误".into()),
    ] {
        let mut accumulator = StreamAccumulator::new();
        accumulator.process(StreamPart::Error {
            message: String::new(),
            raw: payload,
        });
        accumulator.process(finish(UnifiedFinishReason::Length));
        let result = accumulator.finish();
        assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Error);
        assert_eq!(result.finish_reason.raw, Some(expected));
    }
}
