//! 图片、PDF 和复合工具结果的 Responses 输入转换。

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use zach_ai_core::{FileData, ModelError, ToolResultContentBlock, ToolResultOutput};

pub(super) fn file(
    media_type: &str,
    data: &FileData,
    filename: Option<&str>,
) -> Result<Value, ModelError> {
    if let FileData::Text { text } = data {
        return Ok(json!({"type": "input_text", "text": text}));
    }
    let image = media_type.starts_with("image/");
    if !image && media_type != "application/pdf" {
        return Err(ModelError::unsupported(
            media_type,
            Some("当前附件映射支持图片、PDF 与内联文本".into()),
        ));
    }
    let mut part = json!({"type": if image { "input_image" } else { "input_file" }});
    match data {
        FileData::Reference { reference } => {
            let id = reference
                .get("openai")
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ModelError::InvalidRequest("缺少 openai 文件引用".into()))?;
            part["file_id"] = json!(id);
        }
        FileData::Url { url } => {
            part[if image { "image_url" } else { "file_url" }] = json!(url);
        }
        FileData::Data { data } => {
            let encoded = format!("data:{media_type};base64,{}", BASE64.encode(data));
            part[if image { "image_url" } else { "file_data" }] = json!(encoded);
            if !image {
                part["filename"] = json!(filename.unwrap_or("document.pdf"));
            }
        }
        FileData::Text { .. } => unreachable!(),
    }
    Ok(part)
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
            .map(|part| match part {
                ToolResultContentBlock::Text { text, .. } => {
                    Ok(json!({"type": "input_text", "text": text}))
                }
                ToolResultContentBlock::File {
                    media_type,
                    data,
                    filename,
                    ..
                } => file(media_type, data, filename.as_deref()),
                ToolResultContentBlock::Custom { .. } => {
                    Err(ModelError::unsupported("custom_tool_result", None))
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
    }
}
