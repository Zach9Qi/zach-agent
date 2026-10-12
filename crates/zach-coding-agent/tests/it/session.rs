//! 会话：条目日志与投影、v1 样例文件、存储契约；子模块共用的条目构造辅助放在这里

mod fixtures;
mod log;
mod store;

use serde_json::json;
use zach_ai_core::{AssistantPart, Message, ToolPart, ToolResultOutput};
use zach_coding_agent::session::{EntryBody, MessageEntry, MessageOrigin};

/// 用户输入条目
fn input(text: &str) -> EntryBody {
    EntryBody::message(MessageEntry::new(MessageOrigin::Input, Message::user(text)))
}

/// 纯文本回复条目
fn reply(text: &str) -> EntryBody {
    EntryBody::message(MessageEntry::new(
        MessageOrigin::Response,
        Message::assistant(text),
    ))
}

/// 发起若干本地工具调用的回复条目
fn calls(ids: &[&str]) -> EntryBody {
    let content = ids
        .iter()
        .map(|id| AssistantPart::ToolCall {
            tool_call_id: (*id).into(),
            tool_name: "read".into(),
            input: json!({}),
            provider_executed: false,
            provider_options: None,
        })
        .collect();
    EntryBody::message(MessageEntry::new(
        MessageOrigin::Response,
        Message::Assistant {
            content,
            provider_options: None,
        },
    ))
}

/// 若干工具调用的文本结果条目
fn results(ids: &[&str]) -> EntryBody {
    let parts = ids.iter().map(|id| result_part(id, "ok")).collect();
    EntryBody::message(MessageEntry::new(
        MessageOrigin::ToolResults,
        Message::tool(parts),
    ))
}

fn result_part(id: &str, text: &str) -> ToolPart {
    ToolPart::ToolResult {
        tool_call_id: id.into(),
        tool_name: "read".into(),
        output: ToolResultOutput::text(text),
        provider_options: None,
    }
}

/// 工具消息中各结果的 `(tool_call_id, 文本)`
fn tool_outputs(message: &Message) -> Vec<(String, String)> {
    let Message::Tool { content, .. } = message else {
        panic!("不是工具消息: {message:?}");
    };
    content
        .iter()
        .map(|part| match part {
            ToolPart::ToolResult {
                tool_call_id,
                output: ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. },
                ..
            } => (tool_call_id.clone(), value.clone()),
            other => panic!("意外的工具结果: {other:?}"),
        })
        .collect()
}
