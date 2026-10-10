//! 按历史顺序回放 Responses 消息、函数调用与加密推理。

use serde_json::{json, Value};
use zach_ai_core::{AssistantPart, Message, ModelError, ProviderOptions, ToolPart, UserPart};

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
                let mut index = 0;
                while index < content.len() {
                    if matches!(content[index], AssistantPart::Text { .. }) {
                        let (item, consumed) = text_message(&content[index..]);
                        input.push(item);
                        index += consumed;
                    } else {
                        if let Some(item) = assistant_part(&content[index])? {
                            input.push(item);
                        }
                        index += 1;
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

/// 读取文本块回放所需的 openai 元数据。
fn openai_meta(options: &Option<ProviderOptions>) -> Option<&Value> {
    options.as_ref().and_then(|p| p.inner.get("openai"))
}

fn text_item_id(meta: Option<&Value>) -> Option<&str> {
    meta.and_then(|m| m.get("item_id")).and_then(Value::as_str)
}

fn is_refusal(meta: Option<&Value>) -> bool {
    meta.and_then(|m| m.get("refusal")).and_then(Value::as_bool) == Some(true)
}

/// 回放助手文本。首块必须是 Text；带 `item_id` 的相邻文本合并回同一条
/// message 项并保留原始 id，否则无状态回放加密推理时会因 reasoning 项
/// 缺少按 id 配对的后继输出项而被 API 拒绝。
fn text_message(parts: &[AssistantPart]) -> (Value, usize) {
    let AssistantPart::Text {
        text,
        provider_options,
    } = &parts[0]
    else {
        unreachable!("调用方保证首块为文本");
    };
    let meta = openai_meta(provider_options);
    let Some(item_id) = text_item_id(meta) else {
        // 无本协议回放信息的文本（如手工构造或跨厂商历史）保持简单形状。
        let mut message = if is_refusal(meta) {
            json!({"type": "message", "role": "assistant", "status": "completed",
                "content": [{"type": "refusal", "refusal": text}]})
        } else {
            json!({"role": "assistant", "content": text})
        };
        if let Some(phase) = meta.and_then(|m| m.get("phase")).filter(|v| !v.is_null()) {
            message["phase"] = phase.clone();
        }
        return (message, 1);
    };
    let mut content = Vec::new();
    let mut phase = Value::Null;
    let mut consumed = 0;
    for part in parts {
        let AssistantPart::Text {
            text,
            provider_options,
        } = part
        else {
            break;
        };
        let meta = openai_meta(provider_options);
        if text_item_id(meta) != Some(item_id) {
            break;
        }
        content.push(if is_refusal(meta) {
            json!({"type": "refusal", "refusal": text})
        } else {
            json!({"type": "output_text", "text": text})
        });
        if phase.is_null() {
            if let Some(value) = meta.and_then(|m| m.get("phase")).filter(|v| !v.is_null()) {
                phase = value.clone();
            }
        }
        consumed += 1;
    }
    let mut message = json!({"type": "message", "id": item_id, "role": "assistant",
        "status": "completed", "content": content});
    if !phase.is_null() {
        message["phase"] = phase;
    }
    (message, consumed)
}

fn assistant_part(part: &AssistantPart) -> Result<Option<Value>, ModelError> {
    let item = match part {
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
