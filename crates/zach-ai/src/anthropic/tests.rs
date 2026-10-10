//! HTTP 请求构建的内部契约：地址、认证头、beta 标记与正文。

use super::*;
use reqwest::header::AUTHORIZATION;
use serde_json::{json, Value};
use std::collections::HashMap;
use zach_ai_core::{Prompt, ProviderOptions};

fn body(request: &reqwest::Request) -> Value {
    serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap()
}

#[test]
fn request_targets_messages_endpoint_with_api_key_and_version_headers() {
    let model = AnthropicMessagesModel::new("secret", "example")
        .with_base_url("https://example.com/")
        .with_header(
            HeaderName::from_static("anthropic-beta"),
            HeaderValue::from_static("preset-beta"),
        );
    let mut provider_options = ProviderOptions::new();
    provider_options.insert("anthropic", json!({"betas": ["extra-beta", "preset-beta"]}));
    let options = CallOptions {
        prompt: Prompt::new().with_user("你好"),
        headers: Some(HashMap::from([("x-request-id".into(), "test-id".into())])),
        provider_options: Some(provider_options),
        ..Default::default()
    };
    let (request, built) = model.request(&options, true).unwrap();
    assert_eq!(request.url().as_str(), "https://example.com/v1/messages");
    assert_eq!(request.method(), reqwest::Method::POST);
    assert_eq!(request.headers()["x-api-key"], "secret");
    assert!(request.headers()["x-api-key"].is_sensitive());
    assert!(request.headers().get(AUTHORIZATION).is_none());
    assert_eq!(request.headers()["anthropic-version"], API_VERSION);
    assert_eq!(
        request.headers()["anthropic-beta"],
        "preset-beta,extra-beta"
    );
    assert_eq!(request.headers()["x-request-id"], "test-id");
    assert_eq!(request.headers()[ACCEPT], "text/event-stream");
    let body = body(&request);
    assert_eq!(body, built.body);
    assert_eq!(body["stream"], true);
    assert_eq!(body["model"], "example");
    assert_eq!(
        body["messages"][0]["content"],
        json!([{"type": "text", "text": "你好"}])
    );
}

#[test]
fn requests_without_betas_do_not_add_the_beta_header() {
    let model = AnthropicMessagesModel::new("secret", "example");
    let options = CallOptions::new(vec![zach_ai_core::Message::user("你好")]);
    let (request, _) = model.request(&options, false).unwrap();
    assert!(request.headers().get("anthropic-beta").is_none());
    assert_eq!(request.headers()[ACCEPT], "application/json");
    assert_eq!(body(&request)["stream"], false);
}

/// 本地网关、代理等端点不需要鉴权，空 Key 不应阻止请求，也不应发送空凭据头。
#[test]
fn empty_api_key_sends_no_credential_header() {
    let options = CallOptions::new(vec![zach_ai_core::Message::user("你好")]);
    let (request, _) = AnthropicMessagesModel::new("  ", "example")
        .request(&options, false)
        .unwrap();
    assert!(request.headers().get("x-api-key").is_none());
    assert_eq!(request.headers()["anthropic-version"], API_VERSION);
}

/// 非法根地址在构建 reqwest 请求时失败并映射为 `InvalidRequest`；凭据与请求头的非法字符
/// 由传输层用例覆盖，空模型 ID 由请求构建用例覆盖。
#[test]
fn invalid_base_url_fails_before_http_execution() {
    let model = AnthropicMessagesModel::new("secret", "example").with_base_url("not a URL");
    assert!(matches!(
        model.request(&CallOptions::default(), false),
        Err(ModelError::InvalidRequest(_))
    ));
}
