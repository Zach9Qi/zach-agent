//! 工具入参错误、输出错误、panic 兜底与截断响应

use super::{config, executed_flag, one_call, run_with, tool_output};
use crate::support::{echo_tool, fn_tool, text, tool_calls, Script, ScriptedModel, TestHost};
use serde_json::json;
use std::sync::atomic::Ordering;
use zach_agent::{AgentEvent, ToolError};
use zach_ai_core::{Message, ToolPart, ToolResultOutput, UnifiedFinishReason};

#[tokio::test]
async fn unknown_tools_and_malformed_input_report_input_errors() {
    for (name, input, expected) in [
        ("missing", "{}", "不存在"),
        ("echo", "{bad", "JSON 对象"),
        ("echo", "[1,2]", "JSON 对象"),
    ] {
        let model = one_call(name, input);
        let host = TestHost::default();
        let output = run_with(config(model.clone()), vec![echo_tool("echo")], &host).await;

        assert!(host.kinds().contains(&"tool_input_error".to_string()));
        assert!(!host.kinds().contains(&"tool_output_error".to_string()));
        assert!(
            matches!(tool_output(&output), ToolResultOutput::ErrorText { value, .. } if value.contains(expected))
        );
        assert_eq!(model.call_count(), 2);
    }
}

#[tokio::test]
async fn failing_tools_emit_output_errors() {
    let tool = fn_tool("boom", |_, _| async { Err(ToolError::failed("炸了")) }).shared();
    let host = TestHost::default();
    let output = run_with(config(one_call("boom", "{}")), vec![tool], &host).await;
    assert!(host.events().iter().any(
        |e| matches!(e, AgentEvent::ToolOutputError { error_text, .. } if error_text == "炸了")
    ));
    assert_eq!(tool_output(&output), ToolResultOutput::error_text("炸了"));
}

#[tokio::test]
async fn panicking_tools_become_error_results_without_killing_siblings() {
    let panicking = fn_tool("panic", |_, _| async { panic!("工具内部崩溃") }).shared();
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "panic", "{}"), ("c2", "echo", r#"{"a":1}"#)],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("收到")),
    ]);
    let host = TestHost::default();
    let output = run_with(
        config(model.clone()),
        vec![panicking, echo_tool("echo")],
        &host,
    )
    .await;

    let results: Vec<(String, ToolResultOutput)> = output
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            ToolPart::ToolResult {
                tool_call_id,
                output,
                ..
            } => Some((tool_call_id.clone(), output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2, "每个工具调用都应有对应结果");
    assert!(
        matches!(&results[0].1, ToolResultOutput::ErrorText { value, .. } if value.contains("panic") && value.contains("工具内部崩溃")),
        "panic 应转为错误结果: {:?}",
        results[0].1
    );
    assert_eq!(results[1].1, ToolResultOutput::json(json!({"a": 1})));
    assert!(host.kinds().contains(&"tool_output_error".to_string()));
    assert_eq!(host.kinds().last().map(String::as_str), Some("run_finish"));
    assert_eq!(model.call_count(), 2);
}

#[tokio::test]
async fn truncated_responses_do_not_execute_tools() {
    let (executed, tool) = executed_flag();
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "guarded", "{\"p\":")],
            UnifiedFinishReason::Length,
        )),
        Script::Parts(text("重来")),
    ]);
    let host = TestHost::default();
    let output = run_with(config(model), vec![tool], &host).await;
    assert!(!executed.load(Ordering::SeqCst));
    assert!(
        matches!(tool_output(&output), ToolResultOutput::ErrorText { value, .. } if value.contains("长度上限"))
    );
}
