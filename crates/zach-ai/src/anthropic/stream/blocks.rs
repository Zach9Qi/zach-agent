//! 内容块生命周期：文本、思考、工具调用、服务端工具结果与引用来源。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, ProviderMetadata, SourceContent, StreamPart};

use super::super::response::{error, metadata, string};
use super::parser::MessagesStreamParser;

pub(super) struct Block {
    pub(super) id: String,
    pub(super) kind: Kind,
    pub(super) ended: bool,
}

pub(super) enum Kind {
    Text {
        citations: u64,
    },
    Thinking {
        signature: String,
        redacted: bool,
    },
    ToolUse {
        name: String,
        input: String,
        provider_executed: bool,
        metadata: Option<ProviderMetadata>,
    },
    /// 在开始时已整体透出（服务端工具结果、未知块），结束时无事可做。
    Passive,
}

impl MessagesStreamParser {
    pub(super) fn start_block(
        &mut self,
        index: u64,
        block: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        if self.blocks.contains_key(&index) {
            return Err(error(format!("内容块索引 {index} 重复开始"), None));
        }
        let kind = string(block, "type")?;
        let id = index.to_string();
        let state = match kind {
            "text" => {
                parts.push(StreamPart::TextStart {
                    id: id.clone(),
                    provider_metadata: None,
                });
                let text = block["text"].as_str().unwrap_or_default();
                if !text.is_empty() {
                    parts.push(StreamPart::TextDelta {
                        id: id.clone(),
                        delta: text.into(),
                        provider_metadata: None,
                    });
                }
                let mut citations = 0;
                for citation in block["citations"].as_array().into_iter().flatten() {
                    source(&id, &mut citations, citation, parts);
                }
                Kind::Text { citations }
            }
            "thinking" => {
                parts.push(StreamPart::ReasoningStart {
                    id: id.clone(),
                    provider_metadata: None,
                });
                let thinking = block["thinking"].as_str().unwrap_or_default();
                if !thinking.is_empty() {
                    parts.push(StreamPart::ReasoningDelta {
                        id: id.clone(),
                        delta: thinking.into(),
                        provider_metadata: None,
                    });
                }
                Kind::Thinking {
                    signature: block["signature"].as_str().unwrap_or_default().into(),
                    redacted: false,
                }
            }
            "redacted_thinking" => {
                // 正文为空，全部信息在元数据里；累加器会保留只带元数据的空块。
                parts.push(StreamPart::ReasoningStart {
                    id: id.clone(),
                    provider_metadata: metadata(
                        json!({"redacted_thinking": string(block, "data")?}),
                    ),
                });
                Kind::Thinking {
                    signature: String::new(),
                    redacted: true,
                }
            }
            "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                return self.start_tool(index, kind, block, parts);
            }
            other if other.ends_with("_tool_result") => {
                let tool_call_id = string(block, "tool_use_id")?;
                let tool_name = self
                    .tool_name(tool_call_id)
                    .unwrap_or_else(|| other.trim_end_matches("_tool_result").to_owned());
                let result = block["content"].clone();
                let is_error = block["is_error"].as_bool().unwrap_or(false)
                    || result["type"]
                        .as_str()
                        .is_some_and(|kind| kind.ends_with("_error"));
                parts.push(StreamPart::ToolResult {
                    tool_call_id: tool_call_id.into(),
                    tool_name,
                    result,
                    is_error,
                    preliminary: false,
                    dynamic: false,
                    provider_metadata: metadata(json!({"block": block})),
                });
                Kind::Passive
            }
            _ => {
                parts.push(StreamPart::Custom {
                    kind: "anthropic_content_block".into(),
                    data: block.clone(),
                    provider_metadata: None,
                });
                Kind::Passive
            }
        };
        self.blocks.insert(
            index,
            Block {
                id,
                kind: state,
                ended: false,
            },
        );
        Ok(())
    }

    fn start_tool(
        &mut self,
        index: u64,
        kind: &str,
        block: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        let id = string(block, "id")?.to_owned();
        let name = string(block, "name")?.to_owned();
        let provider_executed = kind != "tool_use";
        // 服务端工具保留去掉入参的原始块，便于下一轮按厂商格式回放。
        let tool_metadata = provider_executed.then(|| {
            let mut raw = block.clone();
            if let Some(object) = raw.as_object_mut() {
                object.remove("input");
            }
            metadata(json!({"block": raw}))
        });
        let tool_metadata = tool_metadata.flatten();
        parts.push(StreamPart::ToolInputStart {
            id: id.clone(),
            tool_name: name.clone(),
            provider_executed,
            dynamic: false,
            title: None,
            provider_metadata: tool_metadata.clone(),
        });
        // 非流式响应直接给出完整入参对象；流式开始事件中为空对象，由增量补齐。
        let mut input = String::new();
        if block["input"]
            .as_object()
            .is_some_and(|input| !input.is_empty())
        {
            input = block["input"].to_string();
            parts.push(StreamPart::ToolInputDelta {
                id: id.clone(),
                delta: input.clone(),
                provider_metadata: None,
            });
        }
        self.blocks.insert(
            index,
            Block {
                id,
                kind: Kind::ToolUse {
                    name,
                    input,
                    provider_executed,
                    metadata: tool_metadata,
                },
                ended: false,
            },
        );
        Ok(())
    }

    pub(super) fn delta(
        &mut self,
        index: u64,
        delta: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        let kind = string(delta, "type")?;
        let block = self
            .blocks
            .get_mut(&index)
            .ok_or_else(|| error(format!("增量缺少对应内容块 {index}"), None))?;
        if block.ended {
            return Err(error("已结束的内容块又收到增量", None));
        }
        let id = block.id.clone();
        match (kind, &mut block.kind) {
            ("text_delta", Kind::Text { .. }) => {
                let text = string(delta, "text")?;
                if !text.is_empty() {
                    parts.push(StreamPart::TextDelta {
                        id,
                        delta: text.into(),
                        provider_metadata: None,
                    });
                }
            }
            ("citations_delta", Kind::Text { citations }) => {
                source(&id, citations, &delta["citation"], parts);
            }
            ("thinking_delta", Kind::Thinking { .. }) => {
                let text = string(delta, "thinking")?;
                if !text.is_empty() {
                    parts.push(StreamPart::ReasoningDelta {
                        id,
                        delta: text.into(),
                        provider_metadata: None,
                    });
                }
            }
            ("signature_delta", Kind::Thinking { signature, .. }) => {
                signature.push_str(string(delta, "signature")?);
            }
            ("input_json_delta", Kind::ToolUse { input, .. }) => {
                let partial = string(delta, "partial_json")?;
                input.push_str(partial);
                if !partial.is_empty() {
                    parts.push(StreamPart::ToolInputDelta {
                        id,
                        delta: partial.into(),
                        provider_metadata: None,
                    });
                }
            }
            (
                "text_delta" | "citations_delta" | "thinking_delta" | "signature_delta"
                | "input_json_delta",
                _,
            ) => {
                return Err(error(format!("增量类型 {kind} 与内容块类型不匹配"), None));
            }
            // 未知增量类型按版本策略忽略。
            _ => {}
        }
        Ok(())
    }

    pub(super) fn stop_block(
        &mut self,
        index: u64,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        let block = self
            .blocks
            .get_mut(&index)
            .ok_or_else(|| error(format!("结束事件缺少对应内容块 {index}"), None))?;
        if block.ended {
            return Err(error(format!("内容块索引 {index} 重复结束"), None));
        }
        block.ended = true;
        let id = block.id.clone();
        match &block.kind {
            Kind::Text { .. } => parts.push(StreamPart::TextEnd {
                id,
                provider_metadata: None,
            }),
            Kind::Thinking {
                signature,
                redacted,
            } => parts.push(StreamPart::ReasoningEnd {
                id,
                // 签名只在结束时给出；没有签名的思考块回放会被拒绝，因此不附加空值。
                provider_metadata: (!redacted && !signature.is_empty())
                    .then(|| metadata(json!({"signature": signature})))
                    .flatten(),
            }),
            Kind::ToolUse {
                name,
                input,
                provider_executed,
                metadata,
            } => {
                parts.push(StreamPart::ToolInputEnd {
                    id: id.clone(),
                    provider_metadata: None,
                });
                parts.push(StreamPart::ToolCall {
                    tool_call_id: id,
                    tool_name: name.clone(),
                    input: input.clone(),
                    provider_executed: *provider_executed,
                    dynamic: false,
                    provider_metadata: metadata.clone(),
                });
            }
            Kind::Passive => {}
        }
        Ok(())
    }

    /// 根据服务端工具调用 ID 找回工具名称，供结果块命名。
    fn tool_name(&self, tool_call_id: &str) -> Option<String> {
        self.blocks.values().find_map(|block| match &block.kind {
            Kind::ToolUse { name, .. } if block.id == tool_call_id => Some(name.clone()),
            _ => None,
        })
    }
}

/// 引用映射为统一来源事件：网页检索结果为 URL，文档定位为 Document。
fn source(block_id: &str, counter: &mut u64, citation: &Value, parts: &mut Vec<StreamPart>) {
    let Some(kind) = citation["type"].as_str() else {
        return;
    };
    let id = format!("{block_id}:citation:{counter}");
    *counter += 1;
    let provider_metadata = metadata(json!({"citation": citation}));
    let content = match kind {
        "web_search_result_location" => SourceContent::Url {
            id,
            url: citation["url"].as_str().unwrap_or_default().into(),
            title: citation["title"].as_str().map(str::to_owned),
            provider_metadata,
        },
        "search_result_location" => SourceContent::Url {
            id,
            url: citation["source"].as_str().unwrap_or_default().into(),
            title: citation["title"].as_str().map(str::to_owned),
            provider_metadata,
        },
        _ => SourceContent::Document {
            id,
            media_type: "application/octet-stream".into(),
            title: citation["document_title"]
                .as_str()
                .unwrap_or("引用文档")
                .into(),
            filename: None,
            provider_metadata,
        },
    };
    parts.push(StreamPart::Source(content));
}
