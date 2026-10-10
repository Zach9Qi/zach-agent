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
    let (request, built) = model.request(&options, true).unwrap();
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
    assert_eq!(sent, built.body);
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["model"], "example");
    assert_eq!(
        sent["input"][0]["content"],
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

/// 非法根地址在构建 reqwest 请求时失败并映射为 `InvalidRequest`；凭据与请求头的非法字符
/// 由传输层用例覆盖，空模型 ID 由请求构建用例覆盖。
#[test]
fn invalid_base_url_fails_before_http_execution() {
    let model = OpenAiResponsesModel::new("secret", "example").with_base_url("not a URL");
    assert!(matches!(
        model.request(&CallOptions::default(), false),
        Err(ModelError::InvalidRequest(_))
    ));
}
