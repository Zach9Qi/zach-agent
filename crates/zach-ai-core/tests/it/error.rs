//! 错误体系：类别与可重试性、摘要格式、来源链渲染、可序列化投影的往返

use serde_json::json;
use std::error::Error;
use std::io;
use std::time::Duration;
use zach_ai_core::error::report;
use zach_ai_core::{ErrorInfo, ErrorKind, ModelError, StreamPart};

const ALL_KINDS: [ErrorKind; 10] = [
    ErrorKind::InvalidRequest,
    ErrorKind::Unsupported,
    ErrorKind::Authentication,
    ErrorKind::ContextLengthExceeded,
    ErrorKind::Rejected,
    ErrorKind::RateLimited,
    ErrorKind::Unavailable,
    ErrorKind::Transport,
    ErrorKind::InvalidResponse,
    ErrorKind::Internal,
];

#[test]
fn only_transient_kinds_are_retryable() {
    let retryable: Vec<_> = ALL_KINDS
        .into_iter()
        .filter(|kind| kind.is_retryable())
        .collect();
    assert_eq!(
        retryable,
        [
            ErrorKind::RateLimited,
            ErrorKind::Unavailable,
            ErrorKind::Transport
        ]
    );
    for kind in ALL_KINDS {
        assert_eq!(
            ModelError::new(kind, "x").is_retryable(),
            kind.is_retryable()
        );
    }
}

#[test]
fn display_is_a_one_line_summary_without_the_source() {
    let error = ModelError::transport("读取响应体失败").with_source(io::Error::new(
        io::ErrorKind::ConnectionReset,
        "os error 10054",
    ));
    assert_eq!(error.to_string(), "传输失败: 读取响应体失败");

    let source = error.source().expect("应保留底层错误");
    assert_eq!(
        source.downcast_ref::<io::Error>().map(io::Error::kind),
        Some(io::ErrorKind::ConnectionReset)
    );
}

#[test]
fn summary_includes_provider_and_status_when_present() {
    let error = ModelError::new(ErrorKind::Unavailable, "Overloaded")
        .with_provider("anthropic")
        .with_status(529);
    assert_eq!(
        error.to_string(),
        "[anthropic] 服务暂不可用: Overloaded (HTTP 529)"
    );
    assert_eq!(
        ModelError::new(ErrorKind::Unavailable, "").to_string(),
        "服务暂不可用"
    );
}

/// 防止回归到"Display 拼接内层 + source 再输出一次"的重复链
#[test]
fn report_renders_each_cause_exactly_once() {
    let inner = ModelError::transport("请求 Messages 失败").with_source(io::Error::new(
        io::ErrorKind::TimedOut,
        "connection timed out",
    ));
    let outer = io::Error::other(inner);

    assert_eq!(
        report(&outer),
        "传输失败: 请求 Messages 失败: connection timed out"
    );
}

#[test]
fn unsupported_uses_the_feature_as_code() {
    let error = ModelError::unsupported("reasoning_effort.medium", Some("仅支持 low".into()));
    assert_eq!(error.kind(), ErrorKind::Unsupported);
    assert_eq!(error.code(), Some("reasoning_effort.medium"));
    assert_eq!(
        error.to_string(),
        "不支持的特性: reasoning_effort.medium（仅支持 low）"
    );
    assert_eq!(
        ModelError::unsupported("generate", None).message(),
        "generate"
    );
}

#[test]
fn error_info_round_trip_keeps_kind_context_and_cause_text() {
    let error = ModelError::new(ErrorKind::RateLimited, "slow down")
        .with_provider("openai")
        .with_status(429)
        .with_code("rate_limit_exceeded")
        .with_retry_after(Duration::from_millis(1500))
        .with_raw(json!({ "error": { "code": "rate_limit_exceeded" } }))
        .with_source(io::Error::other("upstream"));

    let info = ErrorInfo::from(&error);
    assert_eq!(info.to_string(), report(&error));
    assert_eq!(info.causes, ["upstream"]);
    assert_eq!(info.retry_after_ms, Some(1500));

    let restored = ModelError::from(info.clone());
    assert_eq!(restored.kind(), ErrorKind::RateLimited);
    assert_eq!(restored.retry_after(), Some(Duration::from_millis(1500)));
    assert_eq!(report(&restored), report(&error));
    assert_eq!(ErrorInfo::from(restored), info);
}

#[test]
fn error_info_serializes_kind_in_snake_case_and_omits_empty_fields() {
    let info = ErrorInfo::new(ErrorKind::ContextLengthExceeded, "too long");
    assert_eq!(
        serde_json::to_value(&info).unwrap(),
        json!({ "kind": "context_length_exceeded", "message": "too long" })
    );
}

#[test]
fn stream_error_part_flattens_error_info_next_to_the_type_tag() {
    let part = StreamPart::Error(ErrorInfo {
        provider: Some("anthropic".into()),
        ..ErrorInfo::new(ErrorKind::Unavailable, "Overloaded")
    });
    let value = serde_json::to_value(&part).unwrap();
    assert_eq!(
        value,
        json!({
            "type": "error",
            "kind": "unavailable",
            "message": "Overloaded",
            "provider": "anthropic"
        })
    );
    assert_eq!(serde_json::from_value::<StreamPart>(value).unwrap(), part);
}
