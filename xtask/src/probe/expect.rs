//! 可组合的声明式断言，每条断言独立输出结果，任一断言失败则联调失败。

mod replay;

use super::{
    config::Mode,
    output::Reporter,
    trace::{Outcome, Trace},
    ProbeResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zach_ai_core::{Message, OutputContent, ToolPart, ToolResultOutput, UnifiedFinishReason};

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Expectation {
    FinishReason {
        value: UnifiedFinishReason,
    },
    TextNonempty {},
    TextContains {
        value: String,
    },
    TextEquals {
        value: String,
    },
    JsonEquals {
        pointer: String,
        value: Value,
    },
    JsonType {
        pointer: String,
        kind: JsonKind,
    },
    Reasoning {
        evidence: ReasoningEvidence,
    },
    Event {
        event: String,
        min: usize,
    },
    ToolCall {
        name: String,
        #[serde(default)]
        input: Option<Value>,
    },
    ToolResult {
        name: String,
        value: Value,
    },
    Steps {
        min: usize,
        max: usize,
    },
    Replay {
        content: replay::Content,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReasoningEvidence {
    Any,
    Summary,
    Tokens,
    Metadata,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum JsonKind {
    Object,
    Array,
    String,
    Number,
    Boolean,
    Null,
}

impl Expectation {
    pub(super) fn validate(&self, mode: Mode) -> ProbeResult<()> {
        match self {
            Self::TextContains { value } if value.is_empty() => {
                Err("text_contains 不能匹配空字符串".into())
            }
            Self::JsonEquals { pointer, .. } | Self::JsonType { pointer, .. }
                if !valid_pointer(pointer) =>
            {
                Err("JSON 断言必须使用有效 JSON Pointer，根节点使用空字符串".into())
            }
            Self::Steps { min, max } if *min == 0 || min > max => {
                Err("steps 必须满足 1 <= min <= max".into())
            }
            Self::Event { event, min }
                if mode == Mode::Generate || *min == 0 || event.is_empty() =>
            {
                Err("event 断言仅用于 stream/agent，且事件名称非空、min 大于零".into())
            }
            Self::ToolResult { .. } | Self::Replay { .. } if mode != Mode::Agent => {
                Err("tool_result/replay 断言仅适用于 agent 模式".into())
            }
            _ => Ok(()),
        }
    }

    fn matches(&self, outcome: &Outcome, trace: &Trace) -> bool {
        let results: Vec<_> = trace
            .calls
            .iter()
            .filter_map(|c| c.result.as_ref())
            .collect();
        let parts: Vec<_> = results.iter().flat_map(|r| &r.content).collect();
        match self {
            Self::FinishReason { value } => outcome.finish_reason == *value,
            Self::TextNonempty {} => !outcome.text.trim().is_empty(),
            Self::TextContains { value } => outcome.text.contains(value),
            Self::TextEquals { value } => outcome.text.trim() == value,
            Self::JsonEquals { pointer, value } => serde_json::from_str::<Value>(&outcome.text)
                .ok()
                .is_some_and(|json| json.pointer(pointer) == Some(value)),
            Self::JsonType { pointer, kind } => serde_json::from_str::<Value>(&outcome.text)
                .ok()
                .and_then(|json| json.pointer(pointer).cloned())
                .is_some_and(|json| match kind {
                    JsonKind::Object => json.is_object(),
                    JsonKind::Array => json.is_array(),
                    JsonKind::String => json.is_string(),
                    JsonKind::Number => json.is_number(),
                    JsonKind::Boolean => json.is_boolean(),
                    JsonKind::Null => json.is_null(),
                }),
            Self::Reasoning { evidence } => {
                let summary = parts.iter().any(|p| {
                    matches!(p, OutputContent::Reasoning { text, .. } if !text.trim().is_empty())
                });
                let metadata = parts.iter().any(|p| {
                    matches!(p, OutputContent::Reasoning { provider_metadata: Some(m), .. } if !m.is_empty())
                });
                let tokens = results
                    .iter()
                    .any(|r| r.usage.output_tokens.reasoning.unwrap_or(0) > 0);
                match evidence {
                    ReasoningEvidence::Any => summary || metadata || tokens,
                    ReasoningEvidence::Summary => summary,
                    ReasoningEvidence::Tokens => tokens,
                    ReasoningEvidence::Metadata => metadata,
                }
            }
            Self::Event { event, min } => trace
                .calls
                .iter()
                .map(|c| c.events.get(event).copied().unwrap_or(0))
                .sum::<usize>()
                >= *min,
            Self::ToolCall { name, input } => parts.iter().any(|part| match part {
                OutputContent::ToolCall {
                    tool_name,
                    input: actual,
                    ..
                } if tool_name == name => input.as_ref().is_none_or(|expected| {
                    serde_json::from_str::<Value>(actual).ok().as_ref() == Some(expected)
                }),
                _ => false,
            }),
            Self::ToolResult { name, value } => outcome.messages.iter().any(|message| {
                matches!(message, Message::Tool { content, .. } if content.iter().any(|p| matches!(
                    p, ToolPart::ToolResult { tool_name, output: ToolResultOutput::Json { value: actual, .. }, .. }
                    if tool_name == name && actual == value
                )))
            }),
            Self::Steps { min, max } => (*min..=*max).contains(&outcome.steps),
            Self::Replay { content } => replay::matches(content, outcome, trace),
        }
    }
}

pub(super) fn verify(
    expect: &[Expectation],
    outcome: &Outcome,
    trace: &Trace,
    reporter: &Reporter,
) -> ProbeResult<()> {
    let mut failed = Vec::new();
    for (index, assertion) in expect.iter().enumerate() {
        let success = assertion.matches(outcome, trace);
        reporter.emit(
            "assertion_result",
            &serde_json::json!({"index": index, "expect": assertion, "success": success}),
        );
        if !success {
            failed.push(index.to_string());
        }
    }
    reporter.check()?;
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "场景断言失败（从 0 开始的索引）: {}，请查看 assertion_result 和实际结果",
            failed.join(", ")
        ))
    }
}

fn valid_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut chars = pointer.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}
