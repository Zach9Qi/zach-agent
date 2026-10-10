//! Chat Completions 非流式响应解析：文本、工具调用、用量、思考链字段与非法响应。

use super::*;
use zach_ai_core::OutputContent;

#[test]
fn non_stream_response_maps_text_tool_calls_and_usage() {
    let value = json!({
        "id":"chatcmpl-1", "object":"chat.completion", "created":1700000000, "model":"gpt-test",
        "choices":[{"index":0,"message":{"role":"assistant","content":"你好","tool_calls":[{"id":"call_1","type":"function","function":{"name":"sum","arguments":"{\"a\":1}"}}]},"finish_reason":"tool_calls"}],
        "usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":2}}
    });
    let result = parse_response("openai", value).unwrap();
    assert_eq!(result.text(), "你好");
    assert_eq!(result.usage.input_tokens.total, Some(10));
    assert_eq!(result.usage.input_tokens.cache_read, Some(2));
    assert!(matches!(
        result.finish_reason.unified,
        zach_ai_core::UnifiedFinishReason::ToolCalls
    ));
    assert!(result.content.iter().any(|item| matches!(item, zach_ai_core::OutputContent::ToolCall { tool_call_id, .. } if tool_call_id == "call_1")));
}

/// 兼容端点在 `message.reasoning_content`（DeepSeek、Qwen）或 `message.reasoning`（OpenRouter、Ollama）
/// 返回思考链，都要以 Reasoning 块透出并排在正文之前；与流式结果的一致性见 `stream/tests/replay.rs`。
#[test]
fn non_stream_response_surfaces_reasoning_from_either_field() {
    for field in ["reasoning_content", "reasoning"] {
        let mut message = json!({"role": "assistant", "content": "结果是 3"});
        message[field] = json!("先算加法");
        let value = json!({
            "id": "chatcmpl-1", "object": "chat.completion", "created": 1700000000, "model": "r1",
            "choices": [{"index": 0, "message": message, "finish_reason": "stop"}]
        });
        let result = parse_response("openai", value).unwrap();
        assert_eq!(result.reasoning().as_deref(), Some("先算加法"), "{field}");
        assert_eq!(result.text(), "结果是 3");
        assert!(matches!(result.content[0], OutputContent::Reasoning { .. }));
    }
}

#[test]
fn incomplete_or_invalid_json_objects_are_not_successful_responses() {
    for response in [
        json!({}),
        json!({"choices": []}),
        json!({"choices": [{"index": 0}]}),
        json!({"choices": [{"index": 0, "message": {"tool_calls": [{"id": "c"}]}}]}),
    ] {
        assert!(
            parse_response("openai", response.clone()).is_err(),
            "{response}"
        );
    }
    // 兼容端点可能省略 finish_reason，按正常结束处理但 raw 为空。
    let lenient = parse_response(
        "openai",
        json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}}]
        }),
    )
    .unwrap();
    assert_eq!(lenient.finish_reason.unified, UnifiedFinishReason::Stop);
    assert_eq!(lenient.finish_reason.raw, None);
    assert_eq!(lenient.text(), "ok");
    let filtered = parse_response(
        "openai",
        json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": null},
                "finish_reason": "content_filter"}]
        }),
    )
    .unwrap();
    assert_eq!(
        filtered.finish_reason.unified,
        UnifiedFinishReason::ContentFilter
    );
    assert!(filtered.content.is_empty());
}
