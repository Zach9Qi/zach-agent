//! 验证初始输入、推理元数据和工具结果确实进入后续模型请求。

use super::super::trace::{Outcome, Trace};
use serde::{Deserialize, Serialize};
use zach_ai_core::{AssistantPart, Message, OutputContent, ToolPart};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::probe) enum Content {
    Input,
    Reasoning,
    ToolResults,
}

pub(super) fn matches(content: &Content, outcome: &Outcome, trace: &Trace) -> bool {
    if trace.calls.len() < 2 {
        return false;
    }
    if matches!(content, Content::Input) {
        let initial = &trace.calls[0].request.prompt.messages;
        return !initial.is_empty()
            && trace.calls[1..]
                .iter()
                .all(|call| call.request.prompt.messages.starts_with(initial));
    }
    let mut observed = false;
    for pair in trace.calls.windows(2) {
        let Some(result) = &pair[0].result else {
            return false;
        };
        let next = &pair[1].request.prompt.messages;
        match content {
            Content::Reasoning => {
                for part in &result.content {
                    if let OutputContent::Reasoning { .. } = part {
                        observed = true;
                        let expected = part.clone().into_assistant_part().expect("推理可回放");
                        if !next.iter().any(|message| {
                            matches!(message,
                            Message::Assistant { content, .. } if content.contains(&expected))
                        }) {
                            return false;
                        }
                    }
                }
            }
            Content::ToolResults => {
                for part in &result.content {
                    let OutputContent::ToolCall {
                        tool_call_id,
                        tool_name,
                        provider_executed: false,
                        ..
                    } = part
                    else {
                        continue;
                    };
                    observed = true;
                    let actual = outcome
                        .messages
                        .iter()
                        .filter_map(|m| match m {
                            Message::Tool { content, .. } => Some(content),
                            _ => None,
                        })
                        .flatten()
                        .find(|p| {
                            matches!(p, ToolPart::ToolResult {
                        tool_call_id: id, tool_name: name, ..
                    } if id == tool_call_id && name == tool_name)
                        });
                    let Some(actual) = actual else {
                        return false;
                    };
                    let has_result = next.iter().any(
                        |m| matches!(m, Message::Tool { content, .. } if content.contains(actual)),
                    );
                    let expected_call = part.clone().into_assistant_part();
                    let has_call = next.iter().any(|m| matches!(m, Message::Assistant { content, .. }
                        if content.iter().any(|p| matches!(p, AssistantPart::ToolCall { .. }) && Some(p) == expected_call.as_ref())));
                    if !has_result || !has_call {
                        return false;
                    }
                }
            }
            Content::Input => unreachable!(),
        }
    }
    observed
}
