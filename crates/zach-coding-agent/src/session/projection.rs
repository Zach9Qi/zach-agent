//! 投影：把当前分支上的条目变成模型上下文
//!
//! 纯函数，不触及 IO。压缩、崩溃修复都只在这里生效，不回写日志；投影是确定的，
//! 同一份日志每次得到相同的上下文。

use super::entry::{CompactionEntry, Entry, EntryBody};
use zach_ai_core::{AssistantPart, Message, ToolPart, ToolResultOutput};

/// 为悬空工具调用补齐结果时的错误文本
pub(crate) const INTERRUPTED: &str = "运行中断，工具未得出结果";

/// 投影当前分支（按时间正序）
pub(crate) fn project(branch: &[&Entry]) -> Vec<Message> {
    let (compaction, visible) = visible(branch);
    let mut messages: Vec<Message> = compaction
        .and_then(|entry| match &entry.body {
            EntryBody::Compaction(compaction) => Some(summary_message(compaction)),
            _ => None,
        })
        .into_iter()
        .collect();
    messages.extend(visible.iter().filter_map(|entry| match &entry.body {
        EntryBody::Message(message) => Some(message.message.clone()),
        _ => None,
    }));
    repair_dangling_calls(messages)
}

/// 当前上下文的组成：最近一次压缩条目（如有），以及仍原样可见的消息条目（按时间正序）
pub(crate) fn visible<'a>(branch: &[&'a Entry]) -> (Option<&'a Entry>, Vec<&'a Entry>) {
    let position = branch
        .iter()
        .rposition(|entry| matches!(entry.body, EntryBody::Compaction(_)));
    let Some(position) = position else {
        return (None, messages_of(branch));
    };
    let EntryBody::Compaction(compaction) = &branch[position].body else {
        unreachable!("位置由 rposition 按压缩条目找到");
    };
    let kept_from = compaction
        .first_kept
        .and_then(|first| branch[..position].iter().position(|e| e.id == first))
        .unwrap_or(position);
    let mut kept = messages_of(&branch[kept_from..position]);
    kept.extend(messages_of(&branch[position + 1..]));
    (Some(branch[position]), kept)
}

/// 压缩摘要以用户消息的形式放在上下文最前
pub(crate) fn summary_message(compaction: &CompactionEntry) -> Message {
    Message::user(format!(
        "以下是此前对话的摘要，更早的内容已被压缩：\n\n<summary>\n{}\n</summary>",
        compaction.summary
    ))
}

/// 其中的消息条目
fn messages_of<'a>(entries: &[&'a Entry]) -> Vec<&'a Entry> {
    entries
        .iter()
        .copied()
        .filter(|entry| matches!(entry.body, EntryBody::Message(_)))
        .collect()
}

/// 发起了本地工具调用、但紧随其后的工具消息没有覆盖全部调用时，补上"运行中断"的错误结果
fn repair_dangling_calls(messages: Vec<Message>) -> Vec<Message> {
    let mut repaired = Vec::with_capacity(messages.len());
    let mut iter = messages.into_iter().peekable();
    while let Some(message) = iter.next() {
        let calls = local_calls(&message);
        repaired.push(message);
        if calls.is_empty() {
            continue;
        }
        let mut results = match iter.next_if(|next| matches!(next, Message::Tool { .. })) {
            Some(Message::Tool {
                content,
                provider_options,
            }) => (content, provider_options),
            _ => (Vec::new(), None),
        };
        for (id, name) in calls {
            let answered = results.0.iter().any(|part| {
                matches!(part, ToolPart::ToolResult { tool_call_id, .. } if *tool_call_id == id)
            });
            if !answered {
                results.0.push(ToolPart::ToolResult {
                    tool_call_id: id,
                    tool_name: name,
                    output: ToolResultOutput::error_text(INTERRUPTED),
                    provider_options: None,
                });
            }
        }
        repaired.push(Message::Tool {
            content: results.0,
            provider_options: results.1,
        });
    }
    repaired
}

/// 助手消息中需要本地执行的工具调用：`(tool_call_id, tool_name)`
fn local_calls(message: &Message) -> Vec<(String, String)> {
    let Message::Assistant { content, .. } = message else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|part| match part {
            AssistantPart::ToolCall {
                tool_call_id,
                tool_name,
                provider_executed: false,
                ..
            } => Some((tool_call_id.clone(), tool_name.clone())),
            _ => None,
        })
        .collect()
}
