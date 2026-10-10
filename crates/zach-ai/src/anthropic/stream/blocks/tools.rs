//! 工具相关内容块：客户端/服务端工具调用的开始，以及服务端工具结果块。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, StreamPart};

use super::super::super::response::{metadata, string};
use super::super::parser::MessagesStreamParser;
use super::{Block, Kind};

impl MessagesStreamParser {
    pub(super) fn start_tool(
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

    /// 服务端工具结果块：按调用 ID 找回工具名，整体以 `ToolResult` 透出并保留原始块供回放。
    pub(super) fn start_tool_result(
        &self,
        kind: &str,
        block: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<Kind, ModelError> {
        let tool_call_id = string(block, "tool_use_id")?;
        let tool_name = self
            .tool_name(tool_call_id)
            .unwrap_or_else(|| kind.trim_end_matches("_tool_result").to_owned());
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
        Ok(Kind::Passive)
    }

    /// 根据服务端工具调用 ID 找回工具名称，供结果块命名。
    fn tool_name(&self, tool_call_id: &str) -> Option<String> {
        self.blocks.values().find_map(|block| match &block.kind {
            Kind::ToolUse { name, .. } if block.id == tool_call_id => Some(name.clone()),
            _ => None,
        })
    }
}
