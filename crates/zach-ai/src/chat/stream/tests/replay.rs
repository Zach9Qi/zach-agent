//! 工具调用、拒绝与思考链在流式与非流式间一致，并能回放到下一轮请求。

use super::*;
use crate::chat::{request::build_request, response::parse_response};
use zach_ai_core::{CallOptions, Message, OutputContent, ToolPart};

/// index 固定为 0 的单个工具调用增量；`id` 与函数名按需携带。
fn tool_call_delta(id: Option<&str>, name: Option<&str>, arguments: &str) -> Value {
    let mut call = json!({"index": 0, "function": {"arguments": arguments}});
    if let Some(id) = id {
        call["id"] = json!(id);
    }
    if let Some(name) = name {
        call["function"]["name"] = json!(name);
    }
    delta(json!({"tool_calls": [call]}), None)
}

/// 并行工具调用的收尾事件必须按厂商给出的 index 顺序发出，不能依赖哈希表遍历顺序。
#[tokio::test]
async fn parallel_tool_calls_finish_in_index_order() {
    let calls = |items: Vec<Value>| delta(json!({"tool_calls": items}), None);
    let wire = data_frame(&calls(vec![
        json!({"index": 0, "id": "call_a", "type": "function", "function": {"name": "add", "arguments": ""}}),
        json!({"index": 1, "id": "call_b", "type": "function", "function": {"name": "sub", "arguments": ""}}),
        json!({"index": 2, "id": "call_c", "type": "function", "function": {"name": "mul", "arguments": ""}}),
    ])) + &data_frame(&calls(vec![
        json!({"index": 2, "function": {"arguments": "{\"c\":3}"}}),
        json!({"index": 0, "function": {"arguments": "{\"a\":1}"}}),
        json!({"index": 1, "function": {"arguments": "{\"b\":2}"}}),
    ])) + &data_frame(&delta(json!({}), Some("tool_calls")))
        + DONE;
    let parts = parse_wire(wire).await;
    let order: Vec<&str> = parts
        .iter()
        .filter_map(|part| match part {
            Ok(StreamPart::ToolCall { tool_call_id, .. }) => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["call_a", "call_b", "call_c"]);
    let result = aggregate(parts);
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::ToolCalls);
    assert!(matches!(
        &result.content[1],
        OutputContent::ToolCall { tool_call_id, tool_name, input, .. }
            if tool_call_id == "call_b" && tool_name == "sub" && input == "{\"b\":2}"
    ));
}

/// 部分兼容端点对先后发起的多次调用复用 index 0，只能靠 id 变化区分：
/// 换 id 时上一个调用必须完整发出，参数不能串到新调用里。
#[tokio::test]
async fn sequential_tool_calls_reusing_the_same_index_are_split_by_id() {
    let call = tool_call_delta;
    let wire = data_frame(&call(Some("call_a"), Some("add"), "{\"a\""))
        + &data_frame(&call(None, None, ":1}"))
        + &data_frame(&call(Some("call_b"), Some("sub"), "{\"b\":2}"))
        + &data_frame(&delta(json!({}), Some("tool_calls")))
        + DONE;
    let parts = parse_wire(wire).await;
    let calls: Vec<(&str, &str, &str)> = parts
        .iter()
        .filter_map(|part| match part {
            Ok(StreamPart::ToolCall {
                tool_call_id,
                tool_name,
                input,
                ..
            }) => Some((tool_call_id.as_str(), tool_name.as_str(), input.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [
            ("call_a", "add", "{\"a\":1}"),
            ("call_b", "sub", "{\"b\":2}")
        ]
    );
    assert_eq!(aggregate(parts).content.len(), 2);
}

/// 防御性用例：协议规定 `id` 随首个增量到达，"参数先到、id 后补"的顺序不见于任何官方文档，
/// 只是为少数兼容端点兜底。参数先缓存，id 到达时随 `ToolInputStart` 一次补发，
/// 任何事件都不能带着空 id 泄漏出去，否则累加器会为空 id 另开一个残缺的槽位。
#[tokio::test]
async fn arguments_arriving_before_the_id_are_buffered_until_the_id_is_known() {
    let wire = data_frame(&tool_call_delta(None, Some("add"), "{\"a\""))
        + &data_frame(&tool_call_delta(Some("call_a"), None, ":1"))
        + &data_frame(&tool_call_delta(None, None, "}"))
        + &data_frame(&delta(json!({}), Some("tool_calls")))
        + DONE;
    let parts = parse_wire(wire).await;
    let tool_events: Vec<(&str, String)> = parts
        .iter()
        .filter_map(|part| match part {
            Ok(StreamPart::ToolInputStart { id, tool_name, .. }) => {
                Some(("start", format!("{id}:{tool_name}")))
            }
            Ok(StreamPart::ToolInputDelta { id, delta, .. }) => {
                Some(("delta", format!("{id}:{delta}")))
            }
            Ok(StreamPart::ToolInputEnd { id, .. }) => Some(("end", id.clone())),
            Ok(StreamPart::ToolCall {
                tool_call_id,
                input,
                ..
            }) => Some(("call", format!("{tool_call_id}:{input}"))),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_events,
        [
            ("start", "call_a:add".to_owned()),
            ("delta", "call_a:{\"a\"".to_owned()),
            ("delta", "call_a::1".to_owned()),
            ("delta", "call_a:}".to_owned()),
            ("end", "call_a".to_owned()),
            ("call", "call_a:{\"a\":1}".to_owned()),
        ]
    );
    let result = aggregate(parts);
    assert_eq!(result.content.len(), 1, "{:?}", result.content);
}

/// 始终没有 id 的工具调用无法与执行结果配对：不能当作正常调用发出，也不能静默消失——
/// finish_reason 仍是 `tool_calls`，上层会误以为模型什么都没调用。
#[tokio::test]
async fn tool_call_without_any_id_poisons_the_turn_instead_of_vanishing() {
    let wire = data_frame(&tool_call_delta(None, Some("add"), "{\"a\":1}"))
        + &data_frame(&delta(json!({}), Some("tool_calls")))
        + DONE;
    let parts = parse_wire(wire).await;
    assert!(parts.iter().all(Result::is_ok), "{parts:?}");
    assert!(!parts
        .iter()
        .any(|p| matches!(p, Ok(StreamPart::ToolCall { .. }))));
    assert!(parts.iter().any(|p| matches!(
        p,
        Ok(StreamPart::Error { message, raw: Some(raw) })
            if message.contains("缺少 id") && raw["name"] == "add"
    )));
    assert_eq!(
        aggregate_lenient(parts).finish_reason.unified,
        UnifiedFinishReason::Error
    );
    // 只有 index 的占位增量不是调用，不报错。
    let wire = data_frame(&delta(json!({"tool_calls": [{"index": 0}]}), None))
        + &data_frame(&delta(json!({"content": "答"}), Some("stop")))
        + DONE;
    assert_eq!(aggregate(parse_wire(wire).await).text(), "答");
}

/// 流式拼接的工具调用与非流式响应得到同样的内容，并按 Chat 消息形状回放到下一轮。
#[tokio::test]
async fn tool_calls_match_the_non_stream_shape_and_replay_into_the_next_request() {
    let wire = data_frame(&delta(
        json!({"role": "assistant", "content": "我来算"}),
        None,
    )) + &data_frame(&delta(
        json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                "function": {"name": "add", "arguments": "{\"a\":"}}]}),
        None,
    )) + &data_frame(&delta(
        json!({"tool_calls": [{"index": 0, "function": {"arguments": "1}"}}]}),
        None,
    )) + &data_frame(&delta(json!({}), Some("tool_calls")))
        + &data_frame(&usage_chunk())
        + DONE;
    let streamed = aggregate(parse_wire(wire).await);
    let generated = parse_response("openai", json!({
        "id": "chatcmpl-1", "object": "chat.completion", "created": 1700000000, "model": "example",
        "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
            "role": "assistant", "content": "我来算",
            "tool_calls": [{"id": "call_1", "type": "function",
                "function": {"name": "add", "arguments": "{\"a\":1}"}}]
        }}],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5,
            "prompt_tokens_details": {"cached_tokens": 1}}
    }))
    .unwrap();
    assert_eq!(streamed.content, generated.content);
    assert_eq!(streamed.finish_reason, generated.finish_reason);
    assert_eq!(streamed.usage, generated.usage);
    assert_eq!(streamed.response, generated.response);

    let options = CallOptions::new(vec![
        Message::user("1+1"),
        streamed.into_assistant_message(),
        Message::tool(vec![ToolPart::result_json(
            "call_1",
            "add",
            json!({"sum": 2}),
        )]),
    ]);
    let body = build_request("example", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(
        body["messages"][1],
        json!({"role": "assistant", "content": "我来算", "tool_calls": [
            {"id": "call_1", "type": "function",
             "function": {"name": "add", "arguments": "{\"a\":1}"}}
        ]})
    );
    assert_eq!(
        body["messages"][2],
        json!({"role": "tool", "tool_call_id": "call_1", "content": "{\"sum\":2}"})
    );
}

/// 拒绝文本以带 `refusal` 标记的文本块透出，并以 `refusal` 字段回放。
#[tokio::test]
async fn refusal_is_marked_in_metadata_and_replayed_as_refusal_field() {
    let wire = data_frame(&delta(
        json!({"role": "assistant", "refusal": "无法"}),
        None,
    )) + &data_frame(&delta(json!({"refusal": "回答"}), None))
        + &data_frame(&delta(json!({}), Some("stop")))
        + DONE;
    let streamed = aggregate(parse_wire(wire).await);
    let generated = parse_response("openai", json!({
        "id": "chatcmpl-1", "object": "chat.completion", "created": 1700000000, "model": "example",
        "choices": [{"index": 0, "finish_reason": "stop",
            "message": {"role": "assistant", "content": null, "refusal": "无法回答"}}]
    }))
    .unwrap();
    assert_eq!(streamed.content, generated.content);
    assert!(matches!(
        &streamed.content[0],
        OutputContent::Text { text, provider_metadata: Some(metadata) }
            if text == "无法回答"
                && metadata.get::<Value>("openai").unwrap()["refusal"] == true
    ));
    let options = CallOptions::new(vec![Message::user("x"), streamed.into_assistant_message()]);
    let body = build_request("example", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(body["messages"][1]["refusal"], "无法回答");
    assert_eq!(body["messages"][1]["content"], "");
}

/// 思考链在流式与非流式下得到相同内容；回放时不回传（兼容端点会拒绝该字段）。
#[tokio::test]
async fn reasoning_content_matches_across_modes_and_is_not_replayed() {
    let wire = data_frame(&delta(json!({"reasoning_content": "先算"}), None))
        + &data_frame(&delta(json!({"content": "3"}), None))
        + &data_frame(&delta(json!({}), Some("stop")))
        + DONE;
    let streamed = aggregate(parse_wire(wire).await);
    let generated = parse_response("openai", json!({
        "id": "chatcmpl-1", "object": "chat.completion", "created": 1700000000, "model": "example",
        "choices": [{"index": 0, "finish_reason": "stop",
            "message": {"role": "assistant", "reasoning_content": "先算", "content": "3"}}]
    }))
    .unwrap();
    assert_eq!(streamed.content, generated.content);
    assert_eq!(streamed.reasoning().as_deref(), Some("先算"));
    assert_eq!(streamed.text(), "3");
    assert!(matches!(
        streamed.content[0],
        OutputContent::Reasoning { .. }
    ));
    let options = CallOptions::new(vec![Message::user("x"), streamed.into_assistant_message()]);
    let body = build_request("example", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(
        body["messages"][1],
        json!({"role": "assistant", "content": "3"})
    );
}
