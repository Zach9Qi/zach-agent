//! Chat Completions 请求映射、非流式结果和 SSE 生命周期测试。

use super::*;
use bytes::Bytes;
use futures::{stream, StreamExt};
use serde_json::{json, Value};
use zach_ai_core::{CallOptions, LanguageModel, Message, StreamAccumulator};

#[test]
fn chat_model_declares_token_usage_without_visible_reasoning() {
    let capabilities = OpenAiChatCompletionsModel::new("secret", "gpt-test").capabilities();
    assert!(capabilities.reasoning.tokens);
    assert!(!capabilities.reasoning.summary);
    assert!(!capabilities.reasoning.stream);
    assert!(!capabilities.reasoning.replay);
}

#[test]
fn request_uses_chat_endpoint_and_openai_message_shapes() {
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test")
        .with_base_url("https://example.test/v1/");
    let options = CallOptions::new(vec![Message::system("规则"), Message::user("你好")]);
    let request = model.request(&options, true).unwrap();
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
    let result = response::parse_response(value).unwrap();
    assert_eq!(result.text(), "你好");
    assert_eq!(result.usage.input_tokens.total, Some(10));
    assert_eq!(result.usage.input_tokens.cache_read, Some(2));
    assert!(matches!(
        result.finish_reason.unified,
        zach_ai_core::UnifiedFinishReason::ToolCalls
    ));
    assert!(result.content.iter().any(|item| matches!(item, zach_ai_core::OutputContent::ToolCall { tool_call_id, .. } if tool_call_id == "call_1")));
}

#[tokio::test]
async fn stream_emits_text_once_and_waits_for_done() {
    let chunks = [
        "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"你\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"好\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-test\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
        "data: [DONE]\n\n",
    ].concat();
    let mut output = chat_stream(
        stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(chunks))]),
        ChatStreamParser::new(false),
    );
    let parts: Vec<_> = output.by_ref().collect().await;
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    let result = accumulator.finish();
    assert_eq!(result.text(), "你好");
    assert_eq!(result.usage.input_tokens.total, Some(3));
    assert!(matches!(
        result.finish_reason.unified,
        zach_ai_core::UnifiedFinishReason::Stop
    ));
}

#[test]
fn chat_model_identity_does_not_expose_api_key() {
    let model = OpenAiChatCompletionsModel::new("private", "custom-model");
    assert_eq!(model.provider(), "openai");
    assert_eq!(model.model_id(), "custom-model");
    assert!(!format!("{model:?}").contains("private"));
}
