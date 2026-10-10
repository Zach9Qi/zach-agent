//! Chat Completions 的 HTTP 请求契约、模型身份与非流式结果；请求参数映射见 `tests/params.rs`，
//! 流式生命周期见 `stream/tests.rs`。

mod params;

use super::*;
use serde_json::{json, Value};
use zach_ai_core::{CallOptions, LanguageModel, Message};

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

#[test]
fn non_stream_response_maps_text_tool_calls_and_usage() {
    let value = json!({
        "id":"chatcmpl-1", "object":"chat.completion", "created":1700000000, "model":"gpt-test",
        "choices":[{"index":0,"message":{"role":"assistant","content":"你好","tool_calls":[{"id":"call_1","type":"function","function":{"name":"sum","arguments":"{\"a\":1}"}}]},"finish_reason":"tool_calls"}],
        "usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":2}}
    });
    let result = response::parse_response("openai", value).unwrap();
    assert_eq!(result.text(), "你好");
    assert_eq!(result.usage.input_tokens.total, Some(10));
    assert_eq!(result.usage.input_tokens.cache_read, Some(2));
    assert!(matches!(
        result.finish_reason.unified,
        zach_ai_core::UnifiedFinishReason::ToolCalls
    ));
    assert!(result.content.iter().any(|item| matches!(item, zach_ai_core::OutputContent::ToolCall { tool_call_id, .. } if tool_call_id == "call_1")));
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
    assert!(OpenAiChatCompletionsModel::new(
        "bad
key", "llama3"
    )
    .request(&options, false)
    .is_err());
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

#[test]
fn chat_model_identity_does_not_expose_api_key() {
    let model = OpenAiChatCompletionsModel::new("private", "custom-model");
    assert_eq!(model.provider(), "openai");
    assert_eq!(model.model_id(), "custom-model");
    assert!(!format!("{model:?}").contains("private"));
}

/// 接入 DeepSeek、Qwen 等兼容端点时声明厂商身份：档案改按该厂商查内置目录，
/// 扩展参数改读该厂商键（未提供时回退 openai 键），回放标记仍是协议层的 openai 键。
#[test]
fn declared_provider_drives_catalog_lookup_and_provider_options_key() {
    use zach_ai_core::ProviderOptions;
    let catalog = crate::ModelCatalog::builtin();
    let deepseek = catalog.provider_models("deepseek")[0];
    let model = OpenAiChatCompletionsModel::new("k", &deepseek.id)
        .with_base_url("https://api.deepseek.com/v1")
        .with_provider("deepseek");
    assert_eq!(model.provider(), "deepseek");
    assert_eq!(model.profile(), Some(deepseek));
    assert!(OpenAiChatCompletionsModel::new("k", &deepseek.id)
        .profile()
        .is_none());
    let mut options = CallOptions::new(vec![Message::user("x")]);
    let mut provider = ProviderOptions::new();
    provider.insert("deepseek", json!({"thinking": {"type": "disabled"}}));
    provider.insert("openai", json!({"ignored": true}));
    options.provider_options = Some(provider);
    let (_, built) = model.request(&options, false).unwrap();
    assert_eq!(built.body["thinking"], json!({"type": "disabled"}));
    assert!(built.body.get("ignored").is_none());
    let mut provider = ProviderOptions::new();
    provider.insert("openai", json!({"enable_thinking": false}));
    options.provider_options = Some(provider);
    let (_, built) = model.request(&options, false).unwrap();
    assert_eq!(built.body["enable_thinking"], false);
}

/// 兼容端点（DeepSeek/Qwen）在 message.reasoning_content 返回思考链，不能被静默丢弃。
#[test]
fn non_stream_response_surfaces_reasoning_content() {
    let value = json!({
        "id":"chatcmpl-1", "object":"chat.completion", "created":1700000000, "model":"r1",
        "choices":[{"index":0,"message":{"role":"assistant","reasoning_content":"先算加法","content":"结果是 3"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}
    });
    let result = response::parse_response("openai", value).unwrap();
    assert_eq!(result.reasoning().as_deref(), Some("先算加法"));
    assert_eq!(result.text(), "结果是 3");
    assert!(matches!(
        result.content[0],
        zach_ai_core::OutputContent::Reasoning { .. }
    ));
    let via_reasoning = json!({
        "id":"chatcmpl-2", "object":"chat.completion", "created":1700000000, "model":"r1",
        "choices":[{"index":0,"message":{"role":"assistant","reasoning":"换个字段","content":"3"},"finish_reason":"stop"}]
    });
    let result = response::parse_response("openai", via_reasoning).unwrap();
    assert_eq!(result.reasoning().as_deref(), Some("换个字段"));
}

/// URL 支持声明必须与请求构建的实际能力一致，防止宿主跳过下载后构建失败。
#[test]
fn url_support_claims_match_request_builder_capabilities() {
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test");
    assert!(model.is_url_supported("image/png", "https://example.com/a.png"));
    assert!(!model.is_url_supported("application/pdf", "https://example.com/a.pdf"));
    assert!(!model.is_url_supported("image/png", "file:///tmp/a.png"));
}
