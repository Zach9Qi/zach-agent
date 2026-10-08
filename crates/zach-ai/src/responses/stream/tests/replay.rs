//! 工具 ID 关联、推理回传和普通响应与流式响应的一致性。

use super::*;
use crate::responses::{request::build_request, response::parse_response};
use zach_ai_core::{CallOptions, Message, OutputContent, ToolPart, UnifiedFinishReason};

#[tokio::test]
async fn interleaved_tool_calls_use_call_ids_and_emit_complete_calls_once() {
    let mut wire = String::new();
    let calls = [
        json!({"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "add", "arguments": "{}"}),
        json!({"type": "function_call", "id": "fc_2", "call_id": "call_2", "name": "sub", "arguments": "{}"}),
    ];
    for call in &calls {
        let mut start = call.clone();
        start["arguments"] = json!("");
        wire.push_str(&frame(
            &json!({"type": "response.output_item.added", "item": start}),
        ));
    }
    for delta in ["{", "}"] {
        for call in &calls {
            wire.push_str(&frame(
                &json!({"type": "response.function_call_arguments.delta",
                "item_id": call["id"], "delta": delta}),
            ));
        }
    }
    for call in &calls {
        wire.push_str(&frame(
            &json!({"type": "response.function_call_arguments.done",
            "item_id": call["id"], "arguments": "{}"}),
        ));
        wire.push_str(&frame(
            &json!({"type": "response.output_item.done", "item": call}),
        ));
    }
    wire.push_str(&frame(
        &json!({"type": "response.completed", "response": response(calls.to_vec())}),
    ));
    let parts = parse_wire(wire).await;
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::ToolCall { .. })))
            .count(),
        2
    );
    let delta_ids = parts
        .iter()
        .filter_map(|part| match part {
            Ok(StreamPart::ToolInputDelta { id, .. }) => Some(id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(delta_ids, ["call_1", "call_2", "call_1", "call_2"]);
    let result = aggregate(parts);
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::ToolCalls);
    assert!(
        matches!(&result.content[0], OutputContent::ToolCall { tool_call_id, input, .. }
        if tool_call_id == "call_1" && input == "{}")
    );
}

#[tokio::test]
async fn encrypted_reasoning_and_call_result_survive_a_second_request() {
    let reasoning = json!({"type": "reasoning", "id": "rs_1", "summary": [
        {"type": "summary_text", "text": "先计算"}
    ], "encrypted_content": "opaque"});
    let call = json!({"type": "function_call", "id": "fc_1", "call_id": "call_1",
        "name": "add", "arguments": "{\"a\":1,\"b\":2}"});
    let response = response(vec![reasoning.clone(), call.clone()]);
    let wire = [
        json!({"type": "response.output_item.added", "item": {"type": "reasoning", "id": "rs_1", "summary": []}}),
        json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "summary_index": 0, "delta": "先"}),
        json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_1", "summary_index": 0, "delta": "计算"}),
        json!({"type": "response.output_item.done", "item": reasoning}),
        json!({"type": "response.output_item.done", "item": call}),
        json!({"type": "response.completed", "response": response}),
    ].iter().map(frame).collect::<String>();
    let result = aggregate(parse_wire(wire).await);
    assert_eq!(result, parse_response(response).unwrap());
    assert_eq!(result.reasoning().as_deref(), Some("先计算"));
    let options = CallOptions::new(vec![
        Message::user("1+2"),
        result.into_assistant_message(),
        Message::tool(vec![ToolPart::result_json(
            "call_1",
            "add",
            json!({"sum":3}),
        )]),
    ]);
    let request = build_request("example", &options, true).unwrap();
    assert_eq!(request["input"][1], reasoning);
    assert_eq!(request["input"][2]["id"], "fc_1");
    assert_eq!(request["input"][2]["call_id"], "call_1");
    assert_eq!(request["input"][3]["type"], "function_call_output");
    assert_eq!(request["input"][3]["call_id"], "call_1");
}

#[tokio::test]
async fn empty_reasoning_and_final_only_outputs_are_kept() {
    let reasoning = json!({"type": "reasoning", "id": "rs_empty", "summary": [], "encrypted_content": "opaque"});
    let response = response(vec![reasoning, message("答案")]);
    let wire = frame(&json!({"type": "response.completed", "response": response}));
    let result = aggregate(parse_wire(wire).await);
    assert_eq!(result.content.len(), 2);
    assert!(
        matches!(&result.content[0], OutputContent::Reasoning { text, provider_metadata: Some(_) } if text.is_empty())
    );
    assert_eq!(result, parse_response(response).unwrap());
}

#[test]
fn refusal_and_citations_are_preserved_without_duplicating_output_items() {
    let mut item = message("答案");
    item["content"][0]["annotations"] = json!([{
        "type": "url_citation", "url": "https://example.com", "title": "来源"
    }]);
    let refusal = json!({"id": "msg_2", "type": "message", "role": "assistant",
        "content": [{"type": "refusal", "refusal": "无法回答"}]});
    let mut parser = ResponsesStreamParser::new(true);
    let mut parts =
        parser.data(&json!({"type": "response.output_item.done", "item": item}).to_string());
    parts.extend(
        parser.data(
            &json!({"type": "response.completed", "response": response(vec![item, refusal])})
                .to_string(),
        ),
    );
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, StreamPart::Raw { .. }))
            .count(),
        2
    );
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, StreamPart::Source(_)))
            .count(),
        1
    );
    let result = aggregate(parts.into_iter().map(Ok).collect());
    assert_eq!(result.text(), "答案无法回答");
}
