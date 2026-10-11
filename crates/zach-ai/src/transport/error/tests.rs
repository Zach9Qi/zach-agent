//! 厂商错误分类：状态码兜底、方言优先、正文回退、`Retry-After` 解析与流内错误。

use super::*;
use reqwest::header::HeaderValue;
use serde_json::json;

/// 只认识 `quota` 一个错误码的测试方言，错误码取自 `code` 字段
const DIALECT: ErrorDialect = ErrorDialect {
    code: test_code,
    kind: test_kind,
};

fn test_code(error: &Value) -> Option<&str> {
    error.get("code")?.as_str()
}

fn test_kind(code: &str, _error: &Value) -> Option<ErrorKind> {
    (code == "quota").then_some(ErrorKind::Rejected)
}

fn classify(status: u16, body: &[u8]) -> ModelError {
    http_error(&DIALECT, "p", status, &HeaderMap::new(), body)
}

#[test]
fn status_codes_fall_back_to_stable_kinds() {
    let body = br#"{"type":"error","error":{"type":"x","message":"failure"}}"#;
    for (status, kind) in [
        (401, ErrorKind::Authentication),
        (403, ErrorKind::Authentication),
        (429, ErrorKind::RateLimited),
        (408, ErrorKind::Unavailable),
        (500, ErrorKind::Unavailable),
        (503, ErrorKind::Unavailable),
        (529, ErrorKind::Unavailable),
        (400, ErrorKind::Rejected),
        (404, ErrorKind::Rejected),
    ] {
        let error = classify(status, body);
        assert_eq!(error.kind(), kind, "HTTP {status}");
        assert_eq!(error.status(), Some(status));
        assert_eq!(error.provider(), Some("p"));
        assert_eq!(error.message(), "failure");
        assert!(error.raw().is_some());
    }
}

/// 状态码 429 也可能是需要充值的配额耗尽：厂商错误码比状态码更能说明该怎么处理
#[test]
fn provider_error_code_takes_precedence_over_the_status() {
    let error = classify(429, br#"{"error":{"code":"quota","message":"top up"}}"#);
    assert_eq!(error.kind(), ErrorKind::Rejected);
    assert_eq!(error.code(), Some("quota"));
    assert!(!error.is_retryable());

    let unknown = classify(429, br#"{"error":{"code":"other","message":"slow down"}}"#);
    assert_eq!(unknown.kind(), ErrorKind::RateLimited);
    assert_eq!(unknown.code(), Some("other"));
}

#[test]
fn non_json_flat_and_string_bodies_keep_the_readable_message() {
    let proxy = classify(502, b"proxy failure");
    assert_eq!(proxy.message(), "proxy failure");
    assert!(proxy.raw().is_none());
    assert_eq!(classify(400, b"").message(), "响应正文为空");
    assert_eq!(classify(400, br#"{"message":"flat"}"#).message(), "flat");
    assert_eq!(
        classify(401, br#"{"error":"Unauthorized"}"#).message(),
        "Unauthorized"
    );
}

#[test]
fn retry_after_prefers_milliseconds_and_ignores_http_dates() {
    let with = |pairs: &[(&'static str, &'static str)]| {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_static(value));
        }
        http_error(&DIALECT, "p", 429, &headers, b"").retry_after()
    };
    assert_eq!(
        with(&[("retry-after-ms", "1500"), ("retry-after", "9")]),
        Some(Duration::from_millis(1500))
    );
    assert_eq!(with(&[("retry-after", "2")]), Some(Duration::from_secs(2)));
    assert_eq!(
        with(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")]),
        None
    );
    assert_eq!(with(&[("retry-after", "-1")]), None);
    assert_eq!(with(&[]), None);
}

/// 流内事件没有状态码兜底：认不出的错误不重试，避免重复发送确定性失败的请求
#[test]
fn stream_error_objects_without_a_known_code_are_not_retried() {
    let known = DIALECT.error("p", &json!({"code": "quota", "message": "充值"}));
    assert_eq!(known.kind(), ErrorKind::Rejected);
    assert_eq!(known.code(), Some("quota"));

    let unknown = DIALECT.error("p", &json!({"message": "出错了"}));
    assert_eq!(unknown.kind(), ErrorKind::Rejected);
    assert_eq!(unknown.message(), "出错了");
    assert_eq!(
        DIALECT.error("p", &Value::Null).message(),
        "厂商未给出错误描述"
    );
}

#[test]
fn document_instead_of_event_stream_is_a_protocol_error_unless_the_provider_says_otherwise() {
    let plain = not_event_stream(
        &DIALECT,
        "p",
        200,
        "application/json",
        br#"{"error":{"message":"not a stream"}}"#,
    );
    assert_eq!(plain.kind(), ErrorKind::InvalidResponse);
    assert!(plain.message().contains("application/json"));
    assert!(plain.message().contains("not a stream"));
    assert!(plain.raw().is_some());

    let classified = not_event_stream(
        &DIALECT,
        "p",
        200,
        "application/json",
        br#"{"error":{"code":"quota","message":"top up"}}"#,
    );
    assert_eq!(classified.kind(), ErrorKind::Rejected);
}
