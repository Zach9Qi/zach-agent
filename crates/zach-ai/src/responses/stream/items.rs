//! 输出项快照归一：函数、推理、文本、拒绝与引用来源。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, SourceContent, StreamPart};

use super::super::response::{metadata, string};
use super::parser::ResponsesStreamParser;

/// 多段推理摘要之间的分隔，流式换段与完成快照使用同一个值，保证两边文本一致。
pub(super) const SUMMARY_SEPARATOR: &str = "\n\n";

/// 推理项的可见正文：`summary[].text` 以空行拼接；没有摘要时（如 gpt-oss 一类开源服务只给
/// 原始推理）退回 `content[].text`（`reasoning_text` 块）。两者都缺失按空正文处理。
pub(super) fn reasoning_text(item: &Value) -> String {
    let texts = |field: &str| -> Vec<&str> {
        item[field]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| part["text"].as_str())
            .collect()
    };
    let summary = texts("summary");
    if summary.is_empty() {
        texts("content").concat()
    } else {
        summary.join(SUMMARY_SEPARATOR)
    }
}

impl ResponsesStreamParser {
    pub(super) fn start_item(
        &mut self,
        item: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        let id = string(item, "id")?;
        if self.items.contains_key(id) {
            return Ok(());
        }
        match string(item, "type")? {
            "function_call" => {
                parts.push(StreamPart::ToolInputStart {
                    id: string(item, "call_id")?.into(),
                    tool_name: string(item, "name")?.into(),
                    provider_executed: false,
                    dynamic: false,
                    title: None,
                    provider_metadata: metadata(json!({"item_id": id})),
                });
            }
            "reasoning" => self.start_block(id, true, None, parts),
            _ => {}
        }
        self.items.insert(id.into(), item.clone());
        Ok(())
    }

    pub(super) fn finish_item(
        &mut self,
        item: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        let id = string(item, "id")?.to_owned();
        if self.completed.contains(&id) {
            return Ok(());
        }
        self.start_item(item, parts)?;
        match string(item, "type")? {
            "function_call" => self.finish_call(item, parts)?,
            "reasoning" => {
                self.finish_block(
                    &id,
                    &reasoning_text(item),
                    true,
                    metadata(json!({"responses_item": item})),
                    parts,
                )?;
            }
            "message" => {
                let content = item["content"].as_array().ok_or_else(|| {
                    ModelError::provider_error("openai", "助手消息缺少 content 数组", None)
                })?;
                for (index, part) in content.iter().enumerate() {
                    self.finish_content(item, index as u64, part, parts)?;
                }
            }
            kind if kind.ends_with("_call") || kind == "mcp_approval_request" => {
                return Err(ModelError::unsupported(
                    kind,
                    Some("暂未适配该服务端工具输出".into()),
                ))
            }
            _ => parts.push(StreamPart::Custom {
                kind: "openai_responses_item".into(),
                data: item.clone(),
                provider_metadata: None,
            }),
        }
        self.completed.insert(id);
        Ok(())
    }

    pub(super) fn finish_call(
        &mut self,
        item: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        let id = string(item, "id")?;
        let arguments = string(item, "arguments")?;
        if let Some(previous) = self.calls_done.get(id) {
            if previous != arguments {
                return Err(ModelError::provider_error(
                    "openai",
                    "工具调用的完成快照不一致",
                    None,
                ));
            }
            return Ok(());
        }
        let call_id = string(item, "call_id")?;
        parts.push(StreamPart::ToolInputEnd {
            id: call_id.into(),
            provider_metadata: None,
        });
        parts.push(StreamPart::ToolCall {
            tool_call_id: call_id.into(),
            tool_name: string(item, "name")?.into(),
            input: arguments.into(),
            provider_executed: false,
            dynamic: false,
            provider_metadata: metadata(json!({"item_id": id})),
        });
        self.calls_done.insert(id.into(), arguments.into());
        Ok(())
    }

    pub(super) fn finish_content(
        &mut self,
        item: &Value,
        index: u64,
        part: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        let item_id = string(item, "id")?;
        let id = format!("{item_id}:{index}");
        let refusal = match string(part, "type")? {
            "output_text" => false,
            "refusal" => true,
            other => {
                return Err(ModelError::unsupported(
                    other,
                    Some("未知的 Responses 正文块".into()),
                ))
            }
        };
        self.finish_block(
            &id,
            string(part, if refusal { "refusal" } else { "text" })?,
            false,
            metadata(json!({"item_id": item_id, "phase": item["phase"], "refusal": refusal})),
            parts,
        )?;
        if let Some(annotations) = part["annotations"].as_array() {
            for (index, annotation) in annotations.iter().enumerate() {
                let source_id = format!("{id}:annotation:{index}");
                if !self.sources.insert(source_id.clone()) {
                    continue;
                }
                if annotation["type"] == "url_citation" {
                    parts.push(StreamPart::Source(SourceContent::Url {
                        id: source_id,
                        url: string(annotation, "url")?.into(),
                        title: annotation["title"].as_str().map(str::to_owned),
                        provider_metadata: metadata(json!({"annotation": annotation})),
                    }));
                } else if annotation["type"] == "file_citation" {
                    parts.push(StreamPart::Source(SourceContent::Document {
                        id: source_id,
                        media_type: "application/octet-stream".into(),
                        title: annotation["filename"].as_str().unwrap_or("引用文件").into(),
                        filename: annotation["filename"].as_str().map(str::to_owned),
                        provider_metadata: metadata(json!({"annotation": annotation})),
                    }));
                }
            }
        }
        Ok(())
    }
}
