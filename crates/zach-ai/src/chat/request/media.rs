//! Chat Completions 多模态输入转换。

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use zach_ai_core::{FileData, ModelError, ToolResultContentBlock, ToolResultOutput};

pub(super) fn input(
    media_type: &str,
    data: &FileData,
    filename: Option<&str>,
) -> Result<Value, ModelError> {
    if let FileData::Text { text } = data {
        return Ok(json!({"type":"text", "text":text}));
    }
    if media_type.starts_with("image/") {
        let url = match data {
            FileData::Url { url } => url.clone(),
            FileData::Data { data } => format!("data:{media_type};base64,{}", BASE64.encode(data)),
            FileData::Reference { .. } => {
                return Err(ModelError::unsupported(
                    "file_reference",
                    Some("Chat Completions 图片输入不支持厂商文件引用".into()),
                ))
            }
            FileData::Text { .. } => unreachable!(),
        };
        return Ok(json!({"type":"image_url", "image_url":{"url":url}}));
    }
    if media_type == "application/pdf" {
        let file = match data {
            FileData::Url { .. } => {
                return Err(ModelError::unsupported(
                    "file_url",
                    Some("Chat Completions 文件输入不接受 URL，请使用 file_id 或 data".into()),
                ))
            }
            FileData::Data { data } => {
                json!({"file_data":format!("data:{media_type};base64,{}", BASE64.encode(data)), "filename":filename.unwrap_or("document.pdf")})
            }
            FileData::Reference { reference } => {
                json!({"file_id":reference.get("openai").ok_or_else(|| ModelError::InvalidRequest("缺少 openai 文件引用".into()))?})
            }
            FileData::Text { .. } => unreachable!(),
        };
        return Ok(json!({"type":"file", "file":file}));
    }
    Err(ModelError::unsupported(
        media_type,
        Some("当前附件映射支持图片、PDF 与内联文本".into()),
    ))
}

pub(super) fn tool_output(output: &ToolResultOutput) -> Result<Value, ModelError> {
    match output {
        ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => {
            Ok(json!(value))
        }
        ToolResultOutput::Json { value, .. } | ToolResultOutput::ErrorJson { value, .. } => {
            Ok(json!(value.to_string()))
        }
        ToolResultOutput::ExecutionDenied { reason, .. } => {
            Ok(json!(reason.as_deref().unwrap_or("工具执行被拒绝")))
        }
        ToolResultOutput::Content { value } => value
            .iter()
            .map(|block| match block {
                ToolResultContentBlock::Text { text, .. } => {
                    Ok(json!({"type":"text", "text":text}))
                }
                ToolResultContentBlock::File {
                    media_type,
                    data,
                    filename,
                    ..
                } => input(media_type, data, filename.as_deref()),
                ToolResultContentBlock::Custom { .. } => {
                    Err(ModelError::unsupported("custom_tool_result", None))
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
    }
}
