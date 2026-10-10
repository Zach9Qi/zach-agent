//! Chat Completions 消息回放：系统、用户、助手与工具消息到 OpenAI 消息形状的映射。

use serde_json::{json, Value};
use zach_ai_core::{AssistantPart, Message, ModelError, ToolPart, UserPart};

use super::media;

pub(super) fn convert(messages: &[Message]) -> Result<Vec<Value>, ModelError> {
    messages.iter().try_fold(Vec::new(), |mut all, item| {
        all.extend(message(item)?);
        Ok(all)
    })
}

fn message(message: &Message) -> Result<Vec<Value>, ModelError> {
    match message {
        Message::System { content, .. } => Ok(vec![json!({"role":"system", "content": content})]),
        Message::User { content, .. } => Ok(vec![
            json!({"role":"user", "content": user_content(content)?}),
        ]),
        Message::Assistant { content, .. } => assistant_message(content),
        Message::Tool { content, .. } => tool_message(content),
    }
}

/// 单个文本块用字符串形态，兼容只接受字符串 `content` 的老端点；多块一律用部件数组，
/// 保留各段文本的边界（与 Responses 适配器一致），不再无分隔地拼成一段。
fn user_content(parts: &[UserPart]) -> Result<Value, ModelError> {
    if let [UserPart::Text { text, .. }] = parts {
        return Ok(Value::String(text.clone()));
    }
    parts
        .iter()
        .map(|part| match part {
            UserPart::Text { text, .. } => Ok(json!({"type":"text", "text":text})),
            UserPart::File {
                media_type,
                data,
                filename,
                ..
            } => media::input(media_type, data, filename.as_deref()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn assistant_message(parts: &[AssistantPart]) -> Result<Vec<Value>, ModelError> {
    let mut content = String::new();
    let mut refusal = None;
    let mut calls = Vec::new();
    for part in parts {
        match part {
            AssistantPart::Text {
                text,
                provider_options,
            } => {
                if provider_options
                    .as_ref()
                    .and_then(|p| p.inner.get("openai"))
                    .and_then(|v| v.get("refusal"))
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    refusal = Some(text.clone());
                } else {
                    content.push_str(text);
                }
            }
            AssistantPart::ToolCall {
                tool_call_id,
                tool_name,
                input,
                provider_executed,
                ..
            } => {
                if *provider_executed {
                    return Err(ModelError::unsupported("provider_executed_tool_call", None));
                }
                calls.push(json!({"id":tool_call_id, "type":"function", "function":{"name":tool_name, "arguments":serde_json::to_string(input)?}}));
            }
            // Chat Completions 的输入消息不接受思考链字段（DeepSeek 等端点
            // 收到 reasoning_content 会直接 400），历史推理不回传。
            AssistantPart::Reasoning { .. } => {}
            AssistantPart::ToolResult { .. }
            | AssistantPart::ReasoningFile { .. }
            | AssistantPart::File { .. }
            | AssistantPart::Custom { .. } => {
                return Err(ModelError::unsupported(
                    "assistant_part",
                    Some("Chat Completions 历史支持文本与函数调用".into()),
                ))
            }
        }
    }
    let mut result = json!({"role":"assistant", "content": if content.is_empty() && !calls.is_empty() { Value::Null } else { json!(content) }});
    if !calls.is_empty() {
        result["tool_calls"] = Value::Array(calls);
    }
    if let Some(refusal) = refusal {
        result["refusal"] = json!(refusal);
    }
    Ok(vec![result])
}

fn tool_message(parts: &[ToolPart]) -> Result<Vec<Value>, ModelError> {
    let mut results = Vec::new();
    for part in parts {
        match part {
            ToolPart::ToolResult {
                tool_call_id,
                output,
                ..
            } => {
                results.push(json!({"role":"tool", "tool_call_id":tool_call_id, "content":media::tool_output(output)?}));
            }
            ToolPart::ToolApprovalResponse { .. } => {
                return Err(ModelError::unsupported("tool_approval_response", None))
            }
        }
    }
    if results.is_empty() {
        Err(ModelError::InvalidRequest("工具消息不能为空".into()))
    } else {
        Ok(results)
    }
}
