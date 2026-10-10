//! HTTP 错误分类、非 JSON 正文回退、内容类型校验与请求头合并规则。

use super::*;

#[test]
fn authentication_rate_limit_overload_and_server_faults_have_stable_categories() {
    let body = br#"{"type":"error","error":{"type":"x","message":"failure"}}"#;
    assert!(matches!(
        classify("p", 401, body),
        ModelError::Authentication(message) if message == "failure"
    ));
    assert!(matches!(
        classify("p", 403, body),
        ModelError::Authentication(_)
    ));
    assert!(matches!(
        classify("p", 429, body),
        ModelError::RateLimit(message) if message == "failure"
    ));
    assert!(matches!(classify("p", 529, body), ModelError::RateLimit(_)));
    // 5xx 与 408 是暂时性故障：保留状态码与原始正文，且可重试。
    assert!(matches!(
        classify("p", 500, body),
        ModelError::ServerError { status: 500, raw: Some(_), provider, .. } if provider == "p"
    ));
    assert!(matches!(classify("p", 502, b"proxy failure"),
        ModelError::ServerError { status: 502, message, raw: None, .. }
            if message == "proxy failure"));
    assert!(classify("p", 503, body).is_retryable());
    assert!(classify("p", 408, body).is_retryable());
    // 4xx（限流与鉴权之外）是对请求本身的拒绝，不可重试。
    assert!(matches!(
        classify("p", 404, body),
        ModelError::ProviderError { raw: Some(_), provider, .. } if provider == "p"
    ));
    assert!(!classify("p", 404, body).is_retryable());
    assert!(classify("p", 400, b"").to_string().contains("HTTP 400"));
    assert!(matches!(classify("p", 400, br#"{"message":"flat"}"#),
        ModelError::ProviderError { message, .. } if message == "flat"));
}

/// 网关把流式请求当普通请求处理时会回 JSON/HTML，正文里的错误信息不能丢；
/// 缺失或八位字节流这类 Content-Type 配置疏漏则放行，由 SSE 解码裁决。
#[tokio::test]
async fn document_content_types_are_rejected_with_the_body_message() {
    let response = |content_type: Option<&str>, body: &'static str| -> Response {
        let mut builder = http::Response::builder().status(200);
        if let Some(content_type) = content_type {
            builder = builder.header(CONTENT_TYPE, content_type);
        }
        builder.body(body).unwrap().into()
    };
    let json = response(
        Some("application/json; charset=utf-8"),
        r#"{"error":{"message":"not a stream"}}"#,
    );
    assert!(matches!(
        require_event_stream(json, "p", "测试", None).await,
        Err(ModelError::ProviderError { provider, message, raw: Some(_) })
            if provider == "p"
                && message.contains("not a stream")
                && message.contains("application/json")
    ));
    for document in ["text/html", "text/plain", "application/problem+json"] {
        assert!(
            require_event_stream(response(Some(document), "<html>"), "p", "测试", None)
                .await
                .is_err(),
            "{document}"
        );
    }
    for lenient in [
        Some("text/event-stream; charset=utf-8"),
        Some("application/octet-stream"),
        None,
    ] {
        assert!(
            require_event_stream(response(lenient, "data: x\n\n"), "p", "测试", None)
                .await
                .is_ok(),
            "{lenient:?}"
        );
    }
}

#[test]
fn extra_headers_override_defaults_and_mark_credentials_sensitive() {
    let mut headers = HeaderMap::new();
    headers.insert("x-a", HeaderValue::from_static("1"));
    let extra = HashMap::from([
        ("x-a".to_owned(), "2".to_owned()),
        ("authorization".to_owned(), "Bearer t".to_owned()),
        ("X-Api-Key".to_owned(), "k".to_owned()),
    ]);
    extend_headers(&mut headers, Some(&extra)).unwrap();
    assert_eq!(headers["x-a"], "2");
    assert!(headers[AUTHORIZATION].is_sensitive());
    assert!(headers["x-api-key"].is_sensitive());
    let bad_name = HashMap::from([("bad\nname".to_owned(), "v".to_owned())]);
    assert!(matches!(
        extend_headers(&mut headers, Some(&bad_name)),
        Err(ModelError::InvalidRequest(_))
    ));
    let bad_value = HashMap::from([("x-b".to_owned(), "v\n".to_owned())]);
    assert!(extend_headers(&mut headers, Some(&bad_value)).is_err());
    assert!(sensitive_header("bad\nkey", "API Key").is_err());
    assert!(sensitive_header("ok", "API Key").unwrap().is_sensitive());
}
