//! 模型流分块与工具结果到运行时事件的映射

use serde_json::json;
use zach_agent::event::{map_stream_part, tool_output_event};
use zach_agent::AgentEvent;
use zach_ai_core::{FileData, FinishReason, SourceContent, StreamPart, ToolResultOutput, Usage};

#[test]
fn text_and_reasoning_parts_map_one_to_one() {
    let event = map_stream_part(&StreamPart::TextDelta {
        id: "t1".into(),
        delta: "你好".into(),
        provider_metadata: None,
    });
    assert_eq!(
        event,
        Some(AgentEvent::TextDelta {
            id: "t1".into(),
            delta: "你好".into(),
            provider_metadata: None,
        })
    );

    let event = map_stream_part(&StreamPart::ReasoningEnd {
        id: "r1".into(),
        provider_metadata: None,
    });
    assert!(matches!(event, Some(AgentEvent::ReasoningFinish { id, .. }) if id == "r1"));
}

#[test]
fn tool_input_stream_maps_start_and_delta_only() {
    let start = map_stream_part(&StreamPart::ToolInputStart {
        id: "c1".into(),
        tool_name: "search".into(),
        provider_executed: false,
        dynamic: false,
        title: None,
        provider_metadata: None,
    });
    assert!(
        matches!(start, Some(AgentEvent::ToolInputStart { tool_call_id, .. }) if tool_call_id == "c1")
    );

    let delta = map_stream_part(&StreamPart::ToolInputDelta {
        id: "c1".into(),
        delta: "{\"q\"".into(),
        provider_metadata: None,
    });
    assert!(
        matches!(delta, Some(AgentEvent::ToolInputDelta { input_text_delta, .. }) if input_text_delta == "{\"q\"")
    );

    let end = StreamPart::ToolInputEnd {
        id: "c1".into(),
        provider_metadata: None,
    };
    assert_eq!(map_stream_part(&end), None);

    let finish = StreamPart::Finish {
        usage: Usage::default(),
        finish_reason: FinishReason::stop(),
        provider_metadata: None,
    };
    assert_eq!(map_stream_part(&finish), None);
}

#[test]
fn provider_tool_results_are_marked_provider_executed() {
    let ok = map_stream_part(&StreamPart::ToolResult {
        tool_call_id: "c1".into(),
        tool_name: "web_search".into(),
        result: json!({ "hits": 3 }),
        is_error: false,
        preliminary: false,
        dynamic: false,
        provider_metadata: None,
    });
    match ok {
        Some(AgentEvent::ToolOutputAvailable {
            output,
            provider_executed,
            ..
        }) => {
            assert!(provider_executed);
            assert_eq!(output, ToolResultOutput::json(json!({ "hits": 3 })));
        }
        other => panic!("unexpected: {other:?}"),
    }

    let err = map_stream_part(&StreamPart::ToolResult {
        tool_call_id: "c2".into(),
        tool_name: "web_search".into(),
        result: json!("超时"),
        is_error: true,
        preliminary: false,
        dynamic: false,
        provider_metadata: None,
    });
    assert!(
        matches!(err, Some(AgentEvent::ToolOutputError { error_text, .. }) if error_text == "超时")
    );
}

#[test]
fn files_sources_and_diagnostics_are_mapped() {
    let file = map_stream_part(&StreamPart::File {
        media_type: "image/png".into(),
        data: FileData::from_url("https://x/a.png"),
        provider_metadata: None,
    });
    assert!(matches!(
        file,
        Some(AgentEvent::FileAttachment { url: Some(url), data: None, .. }) if url == "https://x/a.png"
    ));

    let source = map_stream_part(&StreamPart::Source(SourceContent::Url {
        id: "s1".into(),
        url: "https://x".into(),
        title: None,
        provider_metadata: None,
    }));
    assert!(matches!(source, Some(AgentEvent::SourceUrl { source_id, .. }) if source_id == "s1"));

    let error = map_stream_part(&StreamPart::Error {
        message: "坏分块".into(),
        raw: None,
    });
    assert!(matches!(error, Some(AgentEvent::Custom { kind, .. }) if kind == "model.stream_error"));
}

#[test]
fn tool_output_event_selects_variant_by_payload() {
    let denied = tool_output_event(
        "c".into(),
        ToolResultOutput::denied(Some("不允许")),
        false,
        false,
        None,
    );
    assert!(
        matches!(denied, AgentEvent::ToolOutputDenied { reason: Some(r), .. } if r == "不允许")
    );

    let error = tool_output_event(
        "c".into(),
        ToolResultOutput::error_text("坏了"),
        false,
        false,
        None,
    );
    assert!(matches!(error, AgentEvent::ToolOutputError { .. }));

    let progress = tool_output_event(
        "c".into(),
        ToolResultOutput::error_text("中间态"),
        true,
        false,
        None,
    );
    assert!(matches!(
        progress,
        AgentEvent::ToolOutputAvailable {
            preliminary: true,
            ..
        }
    ));
}
