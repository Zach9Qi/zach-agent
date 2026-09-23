//! 模型流分块与工具结果到运行时事件的映射

use crate::event::types::AgentEvent;
use serde_json::{json, Value};
use zach_ai_core::{FileData, ProviderMetadata, SourceContent, StreamPart, ToolResultOutput};

/// 把一个模型流分块映射为运行时事件
///
/// 以下分块返回 `None`，由循环结合上下文另行处理：
/// - `ToolInputEnd` / `ToolCall`：入参就绪后需先校验再发出 `ToolInputAvailable` 或 `ToolInputError`；
/// - `ToolApprovalRequest`：需要补齐工具名与入参；
/// - `Finish`：结束信息汇总在 `StepFinish` / `RunFinish` 中；
/// - 无警告的 `StreamStart`。
pub fn map_stream_part(part: &StreamPart) -> Option<AgentEvent> {
    let event = match part.clone() {
        StreamPart::StreamStart { warnings } if !warnings.is_empty() => AgentEvent::Custom {
            kind: "model.warnings".to_string(),
            data: json!(warnings),
            provider_metadata: None,
        },
        StreamPart::ResponseMetadata(metadata) => AgentEvent::MessageMetadata {
            message_metadata: json!({ "response": metadata }),
        },
        StreamPart::TextStart {
            id,
            provider_metadata,
        } => AgentEvent::TextStart {
            id,
            provider_metadata,
        },
        StreamPart::TextDelta {
            id,
            delta,
            provider_metadata,
        } => AgentEvent::TextDelta {
            id,
            delta,
            provider_metadata,
        },
        StreamPart::TextEnd {
            id,
            provider_metadata,
        } => AgentEvent::TextFinish {
            id,
            provider_metadata,
        },
        StreamPart::ReasoningStart {
            id,
            provider_metadata,
        } => AgentEvent::ReasoningStart {
            id,
            provider_metadata,
        },
        StreamPart::ReasoningDelta {
            id,
            delta,
            provider_metadata,
        } => AgentEvent::ReasoningDelta {
            id,
            delta,
            provider_metadata,
        },
        StreamPart::ReasoningEnd {
            id,
            provider_metadata,
        } => AgentEvent::ReasoningFinish {
            id,
            provider_metadata,
        },
        StreamPart::ToolInputStart {
            id,
            tool_name,
            provider_executed,
            dynamic,
            title,
            provider_metadata,
        } => AgentEvent::ToolInputStart {
            tool_call_id: id,
            tool_name,
            dynamic,
            provider_executed,
            title,
            tool_metadata: None,
            provider_metadata,
        },
        StreamPart::ToolInputDelta { id, delta, .. } => AgentEvent::ToolInputDelta {
            tool_call_id: id,
            input_text_delta: delta,
        },
        StreamPart::ToolResult {
            tool_call_id,
            result,
            is_error,
            preliminary,
            provider_metadata,
            ..
        } => tool_output_event(
            tool_call_id,
            provider_result_output(is_error, result),
            preliminary,
            true,
            provider_metadata,
        ),
        StreamPart::File {
            media_type,
            data,
            provider_metadata,
        } => {
            let (url, data) = split_file(data);
            AgentEvent::FileAttachment {
                media_type,
                url,
                data,
                provider_metadata,
            }
        }
        StreamPart::ReasoningFile {
            media_type,
            data,
            provider_metadata,
        } => {
            let (url, data) = split_file(data);
            AgentEvent::ReasoningFile {
                media_type,
                url,
                data,
                provider_metadata,
            }
        }
        StreamPart::Source(SourceContent::Url {
            id,
            url,
            title,
            provider_metadata,
        }) => AgentEvent::SourceUrl {
            source_id: id,
            url,
            title,
            provider_metadata,
        },
        StreamPart::Source(SourceContent::Document {
            id,
            media_type,
            title,
            filename,
            provider_metadata,
        }) => AgentEvent::SourceDocument {
            source_id: id,
            media_type,
            title,
            filename,
            provider_metadata,
        },
        StreamPart::Custom {
            kind,
            data,
            provider_metadata,
        } => AgentEvent::Custom {
            kind,
            data,
            provider_metadata,
        },
        StreamPart::Error { message, raw } => AgentEvent::Custom {
            kind: "model.stream_error".to_string(),
            data: json!({ "message": message, "raw": raw }),
            provider_metadata: None,
        },
        StreamPart::Raw { raw_value } => AgentEvent::Custom {
            kind: "model.raw".to_string(),
            data: raw_value,
            provider_metadata: None,
        },
        StreamPart::StreamStart { .. }
        | StreamPart::ToolInputEnd { .. }
        | StreamPart::ToolCall { .. }
        | StreamPart::ToolApprovalRequest { .. }
        | StreamPart::Finish { .. } => return None,
    };
    Some(event)
}

/// 按结果载荷选择工具输出事件：错误 → `ToolOutputError`，拒绝 → `ToolOutputDenied`，
/// 其余 → `ToolOutputAvailable`。中间态结果一律为 `ToolOutputAvailable`。
pub fn tool_output_event(
    tool_call_id: String,
    output: ToolResultOutput,
    preliminary: bool,
    provider_executed: bool,
    provider_metadata: Option<ProviderMetadata>,
) -> AgentEvent {
    if !preliminary {
        match output {
            ToolResultOutput::ErrorText { value, .. } => {
                return tool_error(tool_call_id, value, provider_executed, provider_metadata)
            }
            ToolResultOutput::ErrorJson { value, .. } => {
                let text = value.to_string();
                return tool_error(tool_call_id, text, provider_executed, provider_metadata);
            }
            ToolResultOutput::ExecutionDenied { reason, .. } => {
                return AgentEvent::ToolOutputDenied {
                    tool_call_id,
                    reason,
                }
            }
            _ => {}
        }
    }
    AgentEvent::ToolOutputAvailable {
        tool_call_id,
        output,
        preliminary,
        dynamic: false,
        provider_executed,
        tool_metadata: None,
        provider_metadata,
    }
}

fn tool_error(
    tool_call_id: String,
    error_text: String,
    provider_executed: bool,
    provider_metadata: Option<ProviderMetadata>,
) -> AgentEvent {
    AgentEvent::ToolOutputError {
        tool_call_id,
        error_text,
        dynamic: false,
        provider_executed,
        tool_metadata: None,
        provider_metadata,
    }
}

fn provider_result_output(is_error: bool, result: Value) -> ToolResultOutput {
    match (is_error, result) {
        (false, Value::String(text)) => ToolResultOutput::text(text),
        (false, value) => ToolResultOutput::json(value),
        (true, Value::String(text)) => ToolResultOutput::error_text(text),
        (true, value) => ToolResultOutput::ErrorJson {
            value,
            provider_options: None,
        },
    }
}

fn split_file(data: FileData) -> (Option<String>, Option<FileData>) {
    match data {
        FileData::Url { url } => (Some(url), None),
        other => (None, Some(other)),
    }
}
