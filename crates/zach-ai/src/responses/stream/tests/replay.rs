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
    let request = build_request("example", None, &options, true).unwrap().body;
    assert_eq!(request["input"][1], reasoning);
    assert_eq!(request["input"][2]["id"], "fc_1");
    assert_eq!(request["input"][2]["call_id"], "call_1");
    assert_eq!(request["input"][3]["type"], "function_call_output");
    assert_eq!(request["input"][3]["call_id"], "call_1");
}

/// 多段摘要在流式与完成快照里都以空行分隔，否则各段标题与正文会黏成一行。
#[tokio::test]
async fn multi_part_reasoning_summaries_are_separated_by_blank_lines() {
    let reasoning = json!({"type": "reasoning", "id": "rs_1", "summary": [
        {"type": "summary_text", "text": "**第一段**正文"},
        {"type": "summary_text", "text": "**第二段**正文"}
    ], "encrypted_content": "opaque"});
    let payload = response(vec![reasoning.clone(), message("答")]);
    let summary_event = |kind: &str, index: u64, extra: Value| {
        let mut event = json!({"type": kind, "item_id": "rs_1", "summary_index": index});
        event
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        event
    };
    let wire = [
        json!({"type": "response.output_item.added", "item": {"type": "reasoning", "id": "rs_1", "summary": []}}),
        summary_event("response.reasoning_summary_part.added", 0, json!({"part": {"type": "summary_text", "text": ""}})),
        summary_event("response.reasoning_summary_text.delta", 0, json!({"delta": "**第一段**"})),
        summary_event("response.reasoning_summary_text.delta", 0, json!({"delta": "正文"})),
        summary_event("response.reasoning_summary_text.done", 0, json!({"text": "**第一段**正文"})),
        summary_event("response.reasoning_summary_part.added", 1, json!({"part": {"type": "summary_text", "text": ""}})),
        summary_event("response.reasoning_summary_text.delta", 1, json!({"delta": "**第二段**正文"})),
        json!({"type": "response.output_item.done", "item": reasoning}),
        json!({"type": "response.output_item.done", "item": message("答")}),
        json!({"type": "response.completed", "response": payload}),
    ]
    .iter()
    .map(frame)
    .collect::<String>();
    let result = aggregate(parse_wire(wire).await);
    assert_eq!(
        result.reasoning().as_deref(),
        Some("**第一段**正文\n\n**第二段**正文")
    );
    assert_eq!(result, parse_response(payload).unwrap());
}

/// gpt-oss 一类开源服务没有摘要，推理正文在 `content[].reasoning_text` 与
/// `response.reasoning_text.delta` 里，不能被当作空推理丢掉。
#[tokio::test]
async fn raw_reasoning_text_without_summary_is_exposed() {
    let reasoning = json!({"type": "reasoning", "id": "rs_raw", "summary": [],
        "content": [{"type": "reasoning_text", "text": "原始推理"}]});
    let payload = response(vec![reasoning.clone(), message("答")]);
    let wire = [
        json!({"type": "response.output_item.added", "item": {"type": "reasoning", "id": "rs_raw", "summary": [], "content": []}}),
        json!({"type": "response.reasoning_text.delta", "item_id": "rs_raw", "content_index": 0, "delta": "原始"}),
        json!({"type": "response.reasoning_text.delta", "item_id": "rs_raw", "content_index": 0, "delta": "推理"}),
        json!({"type": "response.output_item.done", "item": reasoning}),
        json!({"type": "response.output_item.done", "item": message("答")}),
        json!({"type": "response.completed", "response": payload}),
    ]
    .iter()
    .map(frame)
    .collect::<String>();
    let result = aggregate(parse_wire(wire).await);
    assert_eq!(result.reasoning().as_deref(), Some("原始推理"));
    assert_eq!(result.text(), "答");
    assert_eq!(result, parse_response(payload).unwrap());
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

/// 部分模型的推理项没有 summary 字段，必须按空摘要处理而非让整轮失败。
#[tokio::test]
async fn reasoning_without_summary_field_is_kept_as_empty_not_an_error() {
    let reasoning = json!({"type": "reasoning", "id": "rs_ns", "encrypted_content": "opaque"});
    let payload = response(vec![reasoning, message("答案")]);
    let wire = frame(&json!({"type": "response.completed", "response": payload.clone()}));
    let result = aggregate(parse_wire(wire).await);
    assert!(matches!(
        &result.content[0],
        OutputContent::Reasoning { text, provider_metadata: Some(_) } if text.is_empty()
    ));
    assert_eq!(result.text(), "答案");
    assert_eq!(result, parse_response(payload).unwrap());
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
