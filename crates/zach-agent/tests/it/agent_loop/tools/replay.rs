//! 被拒绝的工具入参在对话历史中的回放归一

use super::{config, run_with, tool_output};
use crate::support::{echo_tool, text, tool_calls, Script, ScriptedModel, TestHost};
use serde_json::json;
use zach_agent::RunOutput;
use zach_ai_core::{AssistantPart, Message, ToolResultOutput, UnifiedFinishReason};

/// 历史中的工具调用入参
fn replayed_tool_inputs(output: &RunOutput) -> Vec<serde_json::Value> {
    output
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            AssistantPart::ToolCall { input, .. } => Some(input.clone()),
            _ => None,
        })
        .collect()
}

/// 非法、截断或非对象的入参不得原样进入对话记录，否则下一轮请求会被厂商拒绝
#[tokio::test]
async fn rejected_tool_inputs_are_replayed_as_empty_objects() {
    let cases = [
        ("{bad", UnifiedFinishReason::ToolCalls, "JSON 对象"),
        ("[1,2]", UnifiedFinishReason::ToolCalls, "JSON 对象"),
        (r#"{"a":"#, UnifiedFinishReason::Length, "长度上限"),
    ];
    for (input, reason, expected) in cases {
        let model = ScriptedModel::new(vec![
            Script::Parts(tool_calls(&[("c1", "echo", input)], reason)),
            Script::Parts(text("收到")),
        ]);
        let host = TestHost::default();
        let output = run_with(config(model), vec![echo_tool("echo")], &host).await;

        assert_eq!(
            replayed_tool_inputs(&output),
            vec![json!({})],
            "输入 {input:?} 回放后应归一为空对象"
        );
        assert!(
            matches!(tool_output(&output), ToolResultOutput::ErrorText { value, .. } if value.contains(expected)),
            "输入 {input:?} 应配对含 {expected:?} 的错误结果"
        );
    }
}

/// 截断只影响执行策略，不改变回放：完整的入参照常保留
#[tokio::test]
async fn truncated_but_complete_inputs_are_replayed_intact() {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "echo", r#"{"a":1}"#)],
            UnifiedFinishReason::Length,
        )),
        Script::Parts(text("收到")),
    ]);
    let host = TestHost::default();
    let output = run_with(config(model), vec![echo_tool("echo")], &host).await;
    assert_eq!(replayed_tool_inputs(&output), vec![json!({"a": 1})]);
}
