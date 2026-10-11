//! 内容类型校验、非 JSON 响应的诊断与请求头合并规则。错误分类见 `transport/error/tests.rs`。

use super::*;
use zach_ai_core::ErrorKind;

/// 不认识任何厂商错误码，完全按状态码与正文分类
const PLAIN: ErrorDialect = ErrorDialect {
    code: no_code,
    kind: no_kind,
};

fn no_code(_: &Value) -> Option<&str> {
    None
}

fn no_kind(_: &str, _: &Value) -> Option<ErrorKind> {
    None
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
    let error = require_event_stream(json, &PLAIN, "p", "测试", None)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidResponse);
    assert_eq!(error.provider(), Some("p"));
    assert!(error.message().contains("not a stream"));
    assert!(error.message().contains("application/json"));
    assert!(error.raw().is_some());
    for document in ["text/html", "text/plain", "application/problem+json"] {
        assert!(
            require_event_stream(
                response(Some(document), "<html>"),
                &PLAIN,
                "p",
                "测试",
                None
            )
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
            require_event_stream(response(lenient, "data: x\n\n"), &PLAIN, "p", "测试", None)
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
    assert_eq!(
        extend_headers(&mut headers, Some(&bad_name))
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidRequest
    );
    let bad_value = HashMap::from([("x-b".to_owned(), "v\n".to_owned())]);
    assert!(extend_headers(&mut headers, Some(&bad_value)).is_err());
    assert!(sensitive_header("bad\nkey", "API Key").is_err());
    assert!(sensitive_header("ok", "API Key").unwrap().is_sensitive());
}
