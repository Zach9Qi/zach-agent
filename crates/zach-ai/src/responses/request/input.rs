//! 按历史顺序回放 Responses 消息、函数调用与加密推理。

use serde_json::{json, Value};
use zach_ai_core::{AssistantPart, Message, ModelError, ToolPart, UserPart};

use super::media;

pub(super) fn messages(messages: &[Message]) -> Result<Vec<Value>, ModelError> {
    let mut input = Vec::new();
    for message in messages {
        match message {
            Message::System { content, .. } => input.push(json!({
                "role": "system", "content": content,
            })),
            Message::User { content, .. } => {
                let content = content
                    .iter()
                    .map(|part| match part {
                        UserPart::Text { text, .. } => {
                            Ok(json!({"type": "input_text", "text": text}))
                        }
                        UserPart::File {
                            media_type,
                            data,
                            filename,
                            ..
                        } => media::file(media_type, data, filename.as_deref()),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                input.push(json!({"role": "user", "content": content}));
            }
            Message::Assistant { content, .. } => {
                for part in content {
                    if let Some(item) = assistant_part(part)? {
                        input.push(item);
                    }
                }
            }
            Message::Tool { content, .. } => {
                for part in content {
                    match part {
                        ToolPart::ToolResult {
                            tool_call_id,
                            output,
                            ..
                        } => input.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_call_id,
                            "output": media::tool_output(output)?,
                        })),
                        ToolPart::ToolApprovalResponse { .. } => {
                            return Err(ModelError::unsupported("tool_approval_response", None))
                        }
                    }
                }
            }
        }
    }
    Ok(input)
}

fn assistant_part(part: &AssistantPart) -> Result<Option<Value>, ModelError> {
    let item = match part {
        AssistantPart::Text {
            text,
            provider_options,
        } => {
            let metadata = provider_options
                .as_ref()
                .and_then(|p| p.inner.get("openai"));
            let mut message = if metadata
                .and_then(|m| m.get("refusal"))
                .and_then(Value::as_bool)
                == Some(true)
            {
                json!({"type": "message", "role": "assistant", "status": "completed",
                    "content": [{"type": "refusal", "refusal": text}]})
            } else {
                json!({"role": "assistant", "content": text})
            };
            if let Some(phase) = metadata
                .and_then(|m| m.get("phase"))
                .filter(|v| !v.is_null())
            {
                message["phase"] = phase.clone();
            }
            message
        }
        AssistantPart::Reasoning {
            provider_options, ..
        } => {
            // 推理摘要不能冒充助手正文；没有本协议回放信息的外部推理不送回。
            let Some(item) = provider_options
                .as_ref()
                .and_then(|p| p.inner.get("openai"))
                .and_then(|p| p.get("responses_item"))
                .filter(|item| item["type"] == "reasoning")
            else {
                return Ok(None);
            };
            item.clone()
        }
        AssistantPart::ToolCall {
            tool_call_id,
            tool_name,
            input,
            provider_executed,
            provider_options,
        } => {
            if *provider_executed {
                return Err(ModelError::unsupported("provider_executed_tool_call", None));
            }
            let mut call = json!({
                "type": "function_call", "call_id": tool_call_id,
                "name": tool_name, "arguments": serde_json::to_string(input)?,
            });
            if let Some(id) = provider_options
                .as_ref()
                .and_then(|p| p.inner.get("openai"))
                .and_then(|p| p.get("item_id"))
                .and_then(Value::as_str)
            {
                call["id"] = json!(id);
            }
            call
        }
        AssistantPart::Custom { kind, data, .. } if kind == "openai_responses_item" => data.clone(),
        _ => {
            return Err(ModelError::unsupported(
                "assistant_part",
                Some("当前 Responses 适配器支持文本、推理与函数调用历史".into()),
            ))
        }
    };
    Ok(Some(item))
}
