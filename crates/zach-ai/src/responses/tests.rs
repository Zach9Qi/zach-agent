//! HTTP 请求构建的内部契约：地址、认证、头覆盖与正文。

use super::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use zach_ai_core::Prompt;

#[test]
fn request_targets_responses_and_keeps_authorization_sensitive() {
    let model = OpenAiResponsesModel::new("secret", "example")
        .with_base_url("https://example.com/v1/")
        .with_header(
            HeaderName::from_static("openai-organization"),
            HeaderValue::from_static("org"),
        );
    let options = CallOptions {
        prompt: Prompt::new().with_user("你好"),
        headers: Some(HashMap::from([("x-request-id".into(), "test-id".into())])),
        ..Default::default()
    };
    let (request, body) = model.request(&options, true).unwrap();
    assert_eq!(request.url().as_str(), "https://example.com/v1/responses");
    assert_eq!(request.method(), reqwest::Method::POST);
    assert_eq!(request.headers()[AUTHORIZATION], "Bearer secret");
    assert!(request.headers()[AUTHORIZATION].is_sensitive());
    assert_eq!(request.headers()["openai-organization"], "org");
    assert_eq!(request.headers()["x-request-id"], "test-id");
    assert_eq!(
        request.headers()[reqwest::header::ACCEPT],
        "text/event-stream"
    );
    let sent: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(sent, body);
    assert_eq!(body["stream"], true);
    assert_eq!(body["model"], "example");
    assert_eq!(
        body["input"][0]["content"],
        json!([{"type":"input_text", "text":"你好"}])
    );
}

/// 无需鉴权的端点用空 Key 接入，不应被拒绝，也不应发送空的 Authorization。
#[test]
fn empty_api_key_sends_no_authorization_header() {
    let options = CallOptions::new(vec![zach_ai_core::Message::user("你好")]);
    let (request, _) = OpenAiResponsesModel::new("", "example")
        .request(&options, false)
        .unwrap();
    assert!(request.headers().get(AUTHORIZATION).is_none());
}

#[test]
fn invalid_credentials_headers_and_endpoint_fail_before_http_execution() {
    let options = CallOptions::default();
    assert!(OpenAiResponsesModel::new("bad\nkey", "example")
        .request(&options, false)
        .is_err());
    assert!(OpenAiResponsesModel::new("secret", "")
        .request(&options, false)
        .is_err());
    let model = OpenAiResponsesModel::new("secret", "example").with_base_url("not a URL");
    assert!(matches!(
        model.request(&options, false),
        Err(ModelError::InvalidRequest(_))
    ));
    let mut options = options;
    options.headers = Some(HashMap::from([("bad\nname".into(), "value".into())]));
    assert!(OpenAiResponsesModel::new("secret", "example")
        .request(&options, false)
        .is_err());
}
