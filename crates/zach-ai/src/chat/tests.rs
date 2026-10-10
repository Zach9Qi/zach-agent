//! Chat Completions 请求映射、非流式结果和 SSE 生命周期测试。

use super::*;
use bytes::Bytes;
use futures::{stream, StreamExt};
use serde_json::{json, Value};
use zach_ai_core::{CallOptions, LanguageModel, Message, StreamAccumulator};

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

/// 兼容端点（DeepSeek/Qwen）在 message.reasoning_content 返回思考链，不能被静默丢弃。
#[test]
fn non_stream_response_surfaces_reasoning_content() {
    let value = json!({
        "id":"chatcmpl-1", "object":"chat.completion", "created":1700000000, "model":"r1",
        "choices":[{"index":0,"message":{"role":"assistant","reasoning_content":"先算加法","content":"结果是 3"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}
    });
    let result = response::parse_response(value).unwrap();
    assert_eq!(result.reasoning().as_deref(), Some("先算加法"));
    assert_eq!(result.text(), "结果是 3");
    assert!(matches!(
        result.content[0],
        zach_ai_core::OutputContent::Reasoning { .. }
    ));
}

/// 流式 delta.reasoning_content 需以 Reasoning 事件透出，并在结束前正确闭合。
#[tokio::test]
async fn stream_surfaces_reasoning_content_deltas_before_text() {
    let chunks = [
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"r1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"先\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"r1\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"算\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"r1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"3\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"r1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat();
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
    assert_eq!(result.reasoning().as_deref(), Some("先算"));
    assert_eq!(result.text(), "3");
    assert!(matches!(
        result.content[0],
        zach_ai_core::OutputContent::Reasoning { .. }
    ));
}

/// URL 支持声明必须与请求构建的实际能力一致，防止宿主跳过下载后构建失败。
#[test]
fn url_support_claims_match_request_builder_capabilities() {
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test");
    assert!(model.is_url_supported("image/png", "https://example.com/a.png"));
    assert!(!model.is_url_supported("application/pdf", "https://example.com/a.pdf"));
    assert!(!model.is_url_supported("image/png", "file:///tmp/a.png"));
}

/// tool 消息的 content 只接受文本部件，文件附件必须在发送前被拒绝而非生成非法部件。
#[test]
fn tool_result_file_blocks_are_rejected_instead_of_emitting_invalid_parts() {
    use zach_ai_core::{FileData, ToolPart, ToolResultContentBlock, ToolResultOutput};
    let result = |blocks| {
        Message::tool(vec![ToolPart::ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "render".into(),
            output: ToolResultOutput::Content { value: blocks },
            provider_options: None,
        }])
    };
    let image = CallOptions::new(vec![result(vec![ToolResultContentBlock::File {
        media_type: "image/png".into(),
        data: FileData::from_bytes(vec![1, 2, 3]),
        filename: None,
        provider_options: None,
    }])]);
    assert!(matches!(
        request::build_request("gpt-test", &image, false),
        Err(zach_ai_core::ModelError::UnsupportedFeature { .. })
    ));
    let text = CallOptions::new(vec![result(vec![ToolResultContentBlock::File {
        media_type: "text/plain".into(),
        data: FileData::Text {
            text: "日志内容".into(),
        },
        filename: None,
        provider_options: None,
    }])]);
    let body = request::build_request("gpt-test", &text, false).unwrap();
    assert_eq!(
        body["messages"][0]["content"],
        json!([{"type":"text", "text":"日志内容"}])
    );
}

/// Chat Completions 没有 `max` 推理档位，防止非法值直达线上端点返回 400。
#[test]
fn reasoning_max_level_is_rejected_before_sending() {
    let mut options = CallOptions::new(vec![Message::user("你好")]);
    options.reasoning = Some(zach_ai_core::ReasoningEffort::Max);
    assert!(matches!(
        request::build_request("gpt-test", &options, false),
        Err(zach_ai_core::ModelError::UnsupportedFeature { .. })
    ));
    options.reasoning = Some(zach_ai_core::ReasoningEffort::Xhigh);
    let body = request::build_request("gpt-test", &options, false).unwrap();
    assert_eq!(body["reasoning_effort"], "xhigh");
}

/// 服务端在流中下发 `error` 后通常直接断开而不发 `[DONE]`，此时不能再叠加一个
/// 可重试的传输错误，否则上层会把厂商错误误判为传输故障并反复重试。
#[tokio::test]
async fn in_band_error_followed_by_connection_close_is_not_a_transport_error() {
    let chunks = "data: {\"error\":{\"message\":\"invalid request\",\"type\":\"invalid_request_error\"}}\n\n";
    let parts: Vec<_> = chat_stream(
        stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(chunks))]),
        ChatStreamParser::new(false),
    )
    .collect()
    .await;
    assert_eq!(parts.len(), 2);
    assert!(matches!(
        &parts[1],
        Ok(zach_ai_core::StreamPart::Error { message, .. }) if message == "invalid request"
    ));
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    assert_eq!(
        accumulator.finish().finish_reason.unified,
        zach_ai_core::UnifiedFinishReason::Error
    );
}
