//! 压缩计划：估算上下文 token、选择切点、提取读写过的文件
//!
//! 估算以字节数 / 4 计（中英文混排时比字符数 / 4 更接近实际），文件附件按固定大小计。

use crate::session::entry::{CompactionEntry, Entry, EntryBody, EntryId};
use crate::session::projection::visible;
use std::collections::BTreeSet;
use zach_ai_core::{
    AssistantPart, Message, ToolPart, ToolResultContentBlock, ToolResultOutput, UserPart,
};

/// 每个文件附件按此字节数估算（约 1200 token）
const FILE_BYTES: usize = 4800;

/// 一次压缩的计划
pub(crate) struct Plan {
    /// 保留区起点
    pub(crate) first_kept: EntryId,
    /// 要被摘要替代的消息
    pub(crate) summarized: Vec<Message>,
    /// 上一次压缩（新摘要在它的基础上更新）
    pub(crate) previous: Option<CompactionEntry>,
}

/// 估算当前上下文的 token 数
///
/// 以最近一次压缩之后、最后一条带用量的回复为锚点：其输入与输出 token 之和，再加上其后各消息的估算；
/// 没有可用的锚点时整体按字节估算（压缩之前的用量不再反映当前上下文）。
pub(crate) fn estimate_tokens(branch: &[&Entry]) -> u64 {
    let (compaction, visible) = visible(branch);
    let after = compaction.map_or(0, |entry| entry.id);
    let anchor = visible
        .iter()
        .rposition(|entry| entry.id > after && usage_tokens(entry).is_some());
    match anchor {
        Some(index) => {
            let base = usage_tokens(visible[index]).unwrap_or(0);
            base + visible[index + 1..]
                .iter()
                .map(|e| entry_tokens(e))
                .sum::<u64>()
        }
        None => {
            let summary = compaction.map_or(0, |entry| match &entry.body {
                EntryBody::Compaction(compaction) => bytes_to_tokens(compaction.summary.len()),
                _ => 0,
            });
            summary + visible.iter().map(|e| entry_tokens(e)).sum::<u64>()
        }
    }
}

/// 从末尾向前保留约 `keep_recent_tokens` 的消息，其余交给摘要
///
/// 切点只落在用户或助手消息上，绝不把工具调用与其结果分开。可见消息总量不足保留量、
/// 或找不到合法切点时没有可压缩的内容，返回 `None`。
pub(crate) fn plan(branch: &[&Entry], keep_recent_tokens: u64) -> Option<Plan> {
    let (compaction, visible) = visible(branch);
    let mut kept = 0;
    let candidate = (0..visible.len()).rev().find(|&index| {
        kept += entry_tokens(visible[index]);
        kept >= keep_recent_tokens
    })?;
    let starts_turn = |index: usize| {
        matches!(
            message_of(visible[index]),
            Some(Message::User { .. } | Message::Assistant { .. })
        )
    };
    let cut = (candidate.max(1)..visible.len())
        .find(|&index| starts_turn(index))
        .or_else(|| (1..candidate).rev().find(|&index| starts_turn(index)))?;
    Some(Plan {
        first_kept: visible[cut].id,
        summarized: visible[..cut]
            .iter()
            .filter_map(|entry| message_of(entry).cloned())
            .collect(),
        previous: compaction.and_then(|entry| match &entry.body {
            EntryBody::Compaction(compaction) => Some(compaction.clone()),
            _ => None,
        }),
    })
}

/// 从工具调用入参的 `path` 提取读过与改过的文件，与此前的列表合并；改过的文件不再列入读过
pub(crate) fn touched_files(
    messages: &[Message],
    previous: Option<&CompactionEntry>,
    read_tools: &[String],
    write_tools: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut read: BTreeSet<String> = previous
        .map(|entry| entry.read_files.iter().cloned().collect())
        .unwrap_or_default();
    let mut modified: BTreeSet<String> = previous
        .map(|entry| entry.modified_files.iter().cloned().collect())
        .unwrap_or_default();
    for message in messages {
        let Message::Assistant { content, .. } = message else {
            continue;
        };
        for part in content {
            let AssistantPart::ToolCall {
                tool_name, input, ..
            } = part
            else {
                continue;
            };
            let Some(path) = input.get("path").and_then(|path| path.as_str()) else {
                continue;
            };
            if write_tools.contains(tool_name) {
                modified.insert(path.to_string());
            } else if read_tools.contains(tool_name) {
                read.insert(path.to_string());
            }
        }
    }
    let read = read.difference(&modified).cloned().collect();
    (read, modified.into_iter().collect())
}

fn message_of(entry: &Entry) -> Option<&Message> {
    match &entry.body {
        EntryBody::Message(message) => Some(&message.message),
        _ => None,
    }
}

fn usage_tokens(entry: &Entry) -> Option<u64> {
    let EntryBody::Message(message) = &entry.body else {
        return None;
    };
    let usage = message.usage.as_ref()?;
    Some(usage.input_tokens.total.unwrap_or(0) + usage.output_tokens.total.unwrap_or(0))
}

fn entry_tokens(entry: &Entry) -> u64 {
    message_of(entry).map_or(0, |message| bytes_to_tokens(message_bytes(message)))
}

fn bytes_to_tokens(bytes: usize) -> u64 {
    u64::try_from(bytes.div_ceil(4)).unwrap_or(u64::MAX)
}

fn message_bytes(message: &Message) -> usize {
    match message {
        Message::System { content, .. } => content.len(),
        Message::User { content, .. } => content
            .iter()
            .map(|part| match part {
                UserPart::Text { text, .. } => text.len(),
                UserPart::File { .. } => FILE_BYTES,
            })
            .sum(),
        Message::Assistant { content, .. } => content
            .iter()
            .map(|part| match part {
                AssistantPart::Text { text, .. } | AssistantPart::Reasoning { text, .. } => {
                    text.len()
                }
                AssistantPart::ToolCall {
                    tool_name, input, ..
                } => tool_name.len() + input.to_string().len(),
                AssistantPart::ToolResult { output, .. } => output_bytes(output),
                _ => FILE_BYTES,
            })
            .sum(),
        Message::Tool { content, .. } => content
            .iter()
            .map(|part| match part {
                ToolPart::ToolResult { output, .. } => output_bytes(output),
                _ => 0,
            })
            .sum(),
    }
}

fn output_bytes(output: &ToolResultOutput) -> usize {
    match output {
        ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => {
            value.len()
        }
        ToolResultOutput::Json { value, .. } | ToolResultOutput::ErrorJson { value, .. } => {
            value.to_string().len()
        }
        ToolResultOutput::ExecutionDenied { reason, .. } => reason.as_ref().map_or(0, String::len),
        ToolResultOutput::Content { value } => value
            .iter()
            .map(|block| match block {
                ToolResultContentBlock::Text { text, .. } => text.len(),
                ToolResultContentBlock::File { .. } => FILE_BYTES,
                ToolResultContentBlock::Custom { data, .. } => data.to_string().len(),
            })
            .sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::entry::{CompactionReason, MessageEntry, MessageOrigin};
    use crate::session::log::SessionLog;
    use serde_json::json;
    use zach_ai_core::Usage;

    fn message(origin: MessageOrigin, message: Message) -> EntryBody {
        EntryBody::message(MessageEntry::new(origin, message))
    }

    fn call() -> EntryBody {
        let content = vec![AssistantPart::ToolCall {
            tool_call_id: "c1".into(),
            tool_name: "read".into(),
            input: json!({ "path": "a.txt" }),
            provider_executed: false,
            provider_options: None,
        }];
        message(
            MessageOrigin::Response,
            Message::Assistant {
                content,
                provider_options: None,
            },
        )
    }

    fn result(text: &str) -> EntryBody {
        message(
            MessageOrigin::ToolResults,
            Message::tool(vec![ToolPart::ToolResult {
                tool_call_id: "c1".into(),
                tool_name: "read".into(),
                output: ToolResultOutput::text(text),
                provider_options: None,
            }]),
        )
    }

    fn log_of(bodies: Vec<EntryBody>) -> SessionLog {
        let mut log = SessionLog::new();
        for body in bodies {
            log.append(body, 0).unwrap();
        }
        log
    }

    /// 保留量只够装下工具结果时，切点退到发起调用的助手消息，调用与结果一起保留
    #[test]
    fn cut_never_separates_a_tool_call_from_its_result() {
        let log = log_of(vec![
            message(MessageOrigin::Input, Message::user("读文件")),
            call(),
            result(&"x".repeat(400)),
        ]);
        let plan = plan(&log.branch(), 10).unwrap();
        assert_eq!(plan.first_kept, 2);
        assert_eq!(plan.summarized, [Message::user("读文件")]);
    }

    #[test]
    fn estimate_ignores_usage_recorded_before_the_latest_compaction() {
        let mut response = MessageEntry::new(MessageOrigin::Response, Message::assistant("答"));
        response.usage = Some(Usage::simple(9000, 1000));
        let mut log = log_of(vec![
            message(MessageOrigin::Input, Message::user("问")),
            EntryBody::message(response),
        ]);
        assert_eq!(estimate_tokens(&log.branch()), 10_000);

        let compaction = CompactionEntry {
            summary: "摘要".into(),
            first_kept: Some(2),
            tokens_before: 10_000,
            reason: CompactionReason::Manual,
            read_files: Vec::new(),
            modified_files: Vec::new(),
        };
        log.append(EntryBody::Compaction(compaction), 0).unwrap();
        assert!(estimate_tokens(&log.branch()) < 10);
    }
}
