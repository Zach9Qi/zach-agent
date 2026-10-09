//! 用户、助手与工具消息的内容块转换，含图片、PDF、文本文档与工具结果。

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use zach_ai_core::{
    AssistantPart, FileData, ModelError, ToolPart, ToolResultContentBlock, ToolResultOutput,
    UserPart,
};

use super::{anthropic_field, cache_control, with_cache_control};

pub(super) fn user_part(part: &UserPart) -> Result<Value, ModelError> {
    match part {
        UserPart::Text {
            text,
            provider_options,
        } => Ok(with_cache_control(
            json!({"type": "text", "text": text}),
            cache_control(provider_options.as_ref()),
        )),
        UserPart::File {
            media_type,
            data,
            filename,
            provider_options,
        } => Ok(with_cache_control(
            file_block(media_type, data, filename.as_deref())?,
            cache_control(provider_options.as_ref()),
        )),
    }
}

/// 图片映射为 `image` 块，PDF 与纯文本映射为 `document` 块。
pub(super) fn file_block(
    media_type: &str,
    data: &FileData,
    filename: Option<&str>,
) -> Result<Value, ModelError> {
    if media_type.starts_with("image/") {
        let source = match data {
            FileData::Data { data } => {
                json!({"type": "base64", "media_type": media_type, "data": BASE64.encode(data)})
            }
            FileData::Url { url } => json!({"type": "url", "url": url}),
            FileData::Reference { reference } => {
                json!({"type": "file", "file_id": file_id(reference)?})
            }
            FileData::Text { .. } => {
                return Err(ModelError::InvalidRequest(
                    "图片附件不能使用内联文本载荷".into(),
                ))
            }
        };
        return Ok(json!({"type": "image", "source": source}));
    }
    let source = match (media_type, data) {
        ("application/pdf", FileData::Data { data }) => {
            json!({"type": "base64", "media_type": "application/pdf", "data": BASE64.encode(data)})
        }
        ("application/pdf", FileData::Url { url }) => json!({"type": "url", "url": url}),
        ("application/pdf", FileData::Reference { reference }) => {
            json!({"type": "file", "file_id": file_id(reference)?})
        }
        (_, FileData::Text { text }) => text_source(text),
        (kind, FileData::Data { data }) if kind.starts_with("text/") => {
            let text = std::str::from_utf8(data).map_err(|_| {
                ModelError::InvalidRequest(format!("{kind} 附件不是合法的 UTF-8 文本"))
            })?;
            text_source(text)
        }
        (kind, FileData::Reference { reference }) if kind.starts_with("text/") => {
            json!({"type": "file", "file_id": file_id(reference)?})
        }
        _ => {
            return Err(ModelError::unsupported(
                media_type,
                Some("当前附件映射支持图片、PDF 与文本文档".into()),
            ))
        }
    };
    let mut block = json!({"type": "document", "source": source});
    if let Some(filename) = filename {
        block["title"] = json!(filename);
    }
    Ok(block)
}

fn text_source(text: &str) -> Value {
    json!({"type": "text", "media_type": "text/plain", "data": text})
}

fn file_id(reference: &std::collections::HashMap<String, String>) -> Result<&str, ModelError> {
    reference
        .get("anthropic")
        .map(String::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ModelError::InvalidRequest("缺少 anthropic 文件引用".into()))
}

pub(super) fn assistant_part(part: &AssistantPart) -> Result<Option<Value>, ModelError> {
    let block = match part {
        AssistantPart::Text {
            text,
            provider_options,
        } => with_cache_control(
            json!({"type": "text", "text": text}),
            cache_control(provider_options.as_ref()),
        ),
        AssistantPart::Reasoning {
            text,
            provider_options,
        } => {
            let options = provider_options.as_ref();
            if let Some(data) = anthropic_field(options, "redacted_thinking") {
                json!({"type": "redacted_thinking", "data": data})
            } else if let Some(signature) = anthropic_field(options, "signature") {
                json!({"type": "thinking", "thinking": text, "signature": signature})
            } else {
                // 没有签名的思考块（来自其他厂商或被裁剪）无法通过校验，不回放。
                return Ok(None);
            }
        }
        AssistantPart::ToolCall {
            tool_call_id,
            tool_name,
            input,
            provider_executed,
            provider_options,
        } => {
            if *provider_executed {
                // 服务端工具调用按原始块回放，仅补回完整入参。
                let mut block = anthropic_field(provider_options.as_ref(), "block")
                    .filter(|block| block.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "server_tool_use"}));
                block["id"] = json!(tool_call_id);
                block["name"] = json!(tool_name);
                block["input"] = input.clone();
                block
            } else {
                with_cache_control(
                    json!({"type": "tool_use", "id": tool_call_id, "name": tool_name, "input": input}),
                    cache_control(provider_options.as_ref()),
                )
            }
        }
        AssistantPart::ToolResult {
            provider_options, ..
        } => anthropic_field(provider_options.as_ref(), "block")
            .cloned()
            .ok_or_else(|| {
                ModelError::unsupported(
                    "assistant_tool_result",
                    Some("助手消息中的工具结果仅支持回放服务端工具的原始块".into()),
                )
            })?,
        AssistantPart::Custom { kind, data, .. } if kind == "anthropic_content_block" => {
            data.clone()
        }
        _ => {
            return Err(ModelError::unsupported(
                "assistant_part",
                Some("当前 Messages 适配器支持文本、思考、工具调用与服务端工具块历史".into()),
            ))
        }
    };
    Ok(Some(block))
}

pub(super) fn tool_part(part: &ToolPart) -> Result<Value, ModelError> {
    match part {
        ToolPart::ToolResult {
            tool_call_id,
            output,
            provider_options,
            ..
        } => {
            let (content, is_error) = tool_result_content(output)?;
            let mut block =
                json!({"type": "tool_result", "tool_use_id": tool_call_id, "content": content});
            if is_error {
                block["is_error"] = json!(true);
            }
            Ok(with_cache_control(
                block,
                cache_control(provider_options.as_ref()),
            ))
        }
        ToolPart::ToolApprovalResponse { .. } => {
            Err(ModelError::unsupported("tool_approval_response", None))
        }
    }
}

/// 工具结果载荷映射为 `tool_result.content` 与 `is_error`。
fn tool_result_content(output: &ToolResultOutput) -> Result<(Value, bool), ModelError> {
    Ok(match output {
        ToolResultOutput::Text { value, .. } => (json!(value), false),
        ToolResultOutput::Json { value, .. } => (json!(value.to_string()), false),
        ToolResultOutput::ErrorText { value, .. } => (json!(value), true),
        ToolResultOutput::ErrorJson { value, .. } => (json!(value.to_string()), true),
        ToolResultOutput::ExecutionDenied { reason, .. } => {
            (json!(reason.as_deref().unwrap_or("工具执行被拒绝")), true)
        }
        ToolResultOutput::Content { value } => {
            let blocks = value
                .iter()
                .map(|block| match block {
                    ToolResultContentBlock::Text { text, .. } => {
                        Ok(json!({"type": "text", "text": text}))
                    }
                    ToolResultContentBlock::File {
                        media_type,
                        data,
                        filename,
                        ..
                    } => file_block(media_type, data, filename.as_deref()),
                    ToolResultContentBlock::Custom { .. } => {
                        Err(ModelError::unsupported("custom_tool_result", None))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            (Value::Array(blocks), false)
        }
    })
}
