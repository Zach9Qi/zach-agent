//! Chat Completions 的 HTTP 请求契约、超时策略与注入档案的接线；模型身份等公开契约见
//! `tests/it/chat.rs`，请求参数映射见 `request/tests.rs`，非流式解析见 `response/tests.rs`，
//! 流式生命周期见 `stream/tests.rs`。

use super::*;
use crate::test_support::profile;
use serde_json::{json, Value};
use zach_ai_core::{CallOptions, Message};

#[test]
fn request_uses_chat_endpoint_and_openai_message_shapes() {
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test")
        .with_base_url("https://example.test/v1/");
    let options = CallOptions::new(vec![Message::system("规则"), Message::user("你好")]);
    let (request, _) = model.request(&options, true).unwrap();
    assert_eq!(
        request.url().as_str(),
        "https://example.test/v1/chat/completions"
    );
    assert!(request.headers()[AUTHORIZATION].is_sensitive());
    let body: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        body["messages"][0],
        json!({"role":"system", "content":"规则"})
    );
    assert_eq!(
        body["messages"][1],
        json!({"role":"user", "content":"你好"})
    );
    assert_eq!(body["stream_options"]["include_usage"], true);
}

/// Ollama、vLLM 等本地端点不需要 Key，空 Key 不应被拒绝，也不应发送空的 Authorization。
#[test]
fn empty_api_key_sends_no_authorization_header() {
    let options = CallOptions::new(vec![Message::user("你好")]);
    let (request, _) = OpenAiChatCompletionsModel::new("", "llama3")
        .with_base_url("http://localhost:11434/v1")
        .request(&options, false)
        .unwrap();
    assert!(request.headers().get(AUTHORIZATION).is_none());
    assert_eq!(
        request.url().as_str(),
        "http://localhost:11434/v1/chat/completions"
    );
}

/// 非流式请求在生成完成前收不到任何字节，只能按整体时长设限；流式请求的总时长由生成长度
/// 决定，不能套用整体超时，而由驱动循环的空闲守卫约束。
#[test]
fn generate_requests_carry_a_total_timeout_and_stream_requests_do_not() {
    use std::time::Duration;
    let options = CallOptions::new(vec![Message::user("你好")]);
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test");
    let (generate, _) = model.request(&options, false).unwrap();
    assert_eq!(generate.timeout(), Some(&Duration::from_secs(600)));
    let (stream, _) = model.request(&options, true).unwrap();
    assert_eq!(stream.timeout(), None);
    let relaxed = model
        .clone()
        .with_generate_timeout(Some(Duration::from_secs(1800)));
    let (generate, _) = relaxed.request(&options, false).unwrap();
    assert_eq!(generate.timeout(), Some(&Duration::from_secs(1800)));
    let unlimited = model.with_generate_timeout(None);
    let (generate, _) = unlimited.request(&options, false).unwrap();
    assert_eq!(generate.timeout(), None);
}

/// 注入的档案参与请求校验：不支持推理的模型丢弃档位并给出警告。
#[test]
fn injected_profile_drives_request_validation() {
    let gpt41 = profile("openai", "gpt-4.1", None, true);
    let model = OpenAiChatCompletionsModel::new("k", "proxy-model").with_profile(gpt41);
    let options = CallOptions::new(vec![Message::user("x")])
        .with_reasoning(zach_ai_core::ReasoningEffort::High);
    let (_, built) = model.request(&options, false).unwrap();
    assert!(built.body.get("reasoning_effort").is_none());
    assert!(matches!(
        &built.warnings[0],
        zach_ai_core::ModelWarning::Unsupported { feature, .. } if feature == "reasoning"
    ));
}
