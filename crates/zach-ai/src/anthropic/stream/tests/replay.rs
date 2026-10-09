//! 工具调用、思考签名、服务端工具块在流式与非流式间一致，并能回放到下一轮请求。

use super::*;
use crate::anthropic::{request::build_request, response::parse_response};
use zach_ai_core::{CallOptions, Message, OutputContent, ToolPart, UnifiedFinishReason};

#[tokio::test]
async fn tool_input_deltas_are_joined_and_emitted_once_per_block() {
    let mut wire = start_frame();
    for event in [
        json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": "add", "input": {}}}),
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": ""}}),
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{\"a\": 1,"}}),
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": " \"b\": 2}"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_2", "name": "noop", "input": {}}}),
        json!({"type": "content_block_stop", "index": 1}),
    ] {
        wire.push_str(&frame(&event));
    }
    wire.push_str(&stop_frames("tool_use"));
    let parts = parse_wire(wire).await;
    let delta_ids = parts
        .iter()
        .filter_map(|part| match part {
            Ok(StreamPart::ToolInputDelta { id, .. }) => Some(id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(delta_ids, ["toolu_1", "toolu_1"]);
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::ToolCall { .. })))
            .count(),
        2
    );
    let result = aggregate(parts);
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::ToolCalls);
    assert!(
        matches!(&result.content[0], OutputContent::ToolCall { tool_call_id, input, .. }
        if tool_call_id == "toolu_1" && input == "{\"a\": 1, \"b\": 2}")
    );
    assert!(
        matches!(&result.content[1], OutputContent::ToolCall { tool_call_id, input, .. }
        if tool_call_id == "toolu_2" && input.is_empty())
    );
    let replay = result.into_assistant_message();
    let request = build_request(
        "example",
        &CallOptions::new(vec![Message::user("x"), replay]),
        false,
    )
    .unwrap();
    assert_eq!(
        request.body["messages"][1]["content"],
        json!([
            {"type": "tool_use", "id": "toolu_1", "name": "add", "input": {"a": 1, "b": 2}},
            {"type": "tool_use", "id": "toolu_2", "name": "noop", "input": {}}
        ])
    );
}

#[tokio::test]
async fn thinking_signature_and_redacted_blocks_survive_a_second_request() {
    let thinking = json!({"type": "thinking", "thinking": "先计算", "signature": "sig"});
    let redacted = json!({"type": "redacted_thinking", "data": "opaque"});
    let call =
        json!({"type": "tool_use", "id": "toolu_1", "name": "add", "input": {"a": 1, "b": 2}});
    let full = message(vec![thinking, redacted, call], "tool_use");
    let mut wire = start_frame();
    for event in [
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "先"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "计算"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "redacted_thinking", "data": "opaque"}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": "add", "input": {}}}),
        json!({"type": "content_block_delta", "index": 2,
            "delta": {"type": "input_json_delta", "partial_json": "{\"a\":1,\"b\":2}"}}),
        json!({"type": "content_block_stop", "index": 2}),
    ] {
        wire.push_str(&frame(&event));
    }
    wire.push_str(&stop_frames("tool_use"));
    let result = aggregate(parse_wire(wire).await);
    assert_eq!(result, parse_response(full).unwrap());
    assert_eq!(result.reasoning().as_deref(), Some("先计算"));
    assert_eq!(result.content.len(), 3);
    let options = CallOptions::new(vec![
        Message::user("1+2"),
        result.into_assistant_message(),
        Message::tool(vec![ToolPart::result_json(
            "toolu_1",
            "add",
            json!({"sum":3}),
        )]),
    ]);
    let request = build_request("example", &options, true).unwrap();
    assert_eq!(
        request.body["messages"][1]["content"],
        json!([
            {"type": "thinking", "thinking": "先计算", "signature": "sig"},
            {"type": "redacted_thinking", "data": "opaque"},
            {"type": "tool_use", "id": "toolu_1", "name": "add", "input": {"a": 1, "b": 2}}
        ])
    );
    assert_eq!(
        request.body["messages"][2]["content"][0]["type"],
        "tool_result"
    );
    assert_eq!(
        request.body["messages"][2]["content"][0]["tool_use_id"],
        "toolu_1"
    );
}

#[test]
fn server_tool_blocks_and_citations_are_preserved_and_replayed() {
    let search = json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
        "input": {"query": "天气"}});
    let search_result = json!({"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
        "content": [{"type": "web_search_result", "url": "https://example.com", "title": "来源"}]});
    let text = json!({"type": "text", "text": "晴", "citations": [
        {"type": "web_search_result_location", "url": "https://example.com", "title": "来源",
            "cited_text": "晴", "encrypted_index": "x"}
    ]});
    let result = parse_response(message(
        vec![search.clone(), search_result.clone(), text],
        "end_turn",
    ))
    .unwrap();
    assert!(
        matches!(&result.content[0], OutputContent::ToolCall { provider_executed: true, tool_name, .. }
        if tool_name == "web_search")
    );
    assert!(
        matches!(&result.content[1], OutputContent::ToolResult { tool_name, is_error: false, .. }
        if tool_name == "web_search")
    );
    assert!(matches!(&result.content[2], OutputContent::Text { text, .. } if text == "晴"));
    assert!(matches!(&result.content[3], OutputContent::Source(_)));
    assert_eq!(result.text(), "晴");
    let request = build_request(
        "example",
        &CallOptions::new(vec![Message::user("x"), result.into_assistant_message()]),
        false,
    )
    .unwrap();
    assert_eq!(
        request.body["messages"][1]["content"],
        json!([search, search_result, {"type": "text", "text": "晴"}])
    );
}

#[test]
fn empty_text_blocks_are_dropped_but_raw_events_are_observable() {
    let mut parser = MessagesStreamParser::new(true);
    let parts = parser.data(
        &json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
            .to_string(),
    );
    assert!(matches!(parts[0], StreamPart::Raw { .. }));
    let mut all = parts;
    all.extend(parser.data(&json!({"type": "content_block_stop", "index": 0}).to_string()));
    all.extend(parser.data(
        &json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}})
            .to_string(),
    ));
    all.extend(parser.data(&json!({"type": "message_stop"}).to_string()));
    assert!(parser.terminal);
    let result = aggregate(all.into_iter().map(Ok).collect());
    assert!(result.content.is_empty());
    assert_eq!(result.usage.output_tokens.total, Some(1));
}
