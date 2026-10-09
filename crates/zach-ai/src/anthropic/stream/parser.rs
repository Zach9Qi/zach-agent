//! Messages 语义事件状态机：按内容块索引维护生命周期，`message_delta` 收尾。

use serde_json::{json, Value};
use std::collections::HashMap;
use zach_ai_core::{ModelError, StreamPart};

use super::super::response::{error, finish_reason, metadata, response_metadata, string, usage};
use super::blocks::Block;

pub(in crate::anthropic) struct MessagesStreamParser {
    pub(super) blocks: HashMap<u64, Block>,
    /// `message_start` 给出的用量，后续 `message_delta` 的累计字段覆盖其中同名项。
    usage: Option<Value>,
    finished: bool,
    pub(super) terminal: bool,
    pub(super) failed: bool,
    raw: bool,
}

impl MessagesStreamParser {
    pub(in crate::anthropic) fn new(raw: bool) -> Self {
        Self {
            blocks: HashMap::new(),
            usage: None,
            finished: false,
            terminal: false,
            failed: false,
            raw,
        }
    }

    pub(super) fn data(&mut self, data: &str) -> Vec<StreamPart> {
        let mut parts = Vec::new();
        match serde_json::from_str::<Value>(data) {
            Ok(event) => {
                if self.raw {
                    parts.push(StreamPart::Raw {
                        raw_value: event.clone(),
                    });
                }
                if let Err(error) = self.event(&event, &mut parts) {
                    self.failed = true;
                    parts.push(StreamPart::Error {
                        message: error.to_string(),
                        raw: Some(event),
                    });
                }
            }
            Err(error) => {
                self.failed = true;
                parts.push(StreamPart::Error {
                    message: format!("Messages SSE JSON 解析失败: {error}"),
                    raw: Some(json!(data)),
                });
            }
        }
        parts
    }

    fn event(&mut self, event: &Value, parts: &mut Vec<StreamPart>) -> Result<(), ModelError> {
        match string(event, "type")? {
            "message_start" => {
                let message = &event["message"];
                parts.push(StreamPart::ResponseMetadata(response_metadata(message)));
                self.merge_usage(&message["usage"]);
            }
            "content_block_start" => {
                self.start_block(index(event)?, &event["content_block"], parts)?
            }
            "content_block_delta" => self.delta(index(event)?, &event["delta"], parts)?,
            "content_block_stop" => self.stop_block(index(event)?, parts)?,
            "message_delta" => {
                if let Some(stop_reason) = event["delta"]["stop_reason"].as_str() {
                    self.finish(stop_reason, &event["delta"], &event["usage"], parts)?;
                } else {
                    // 中途的进度用量只累计，等待携带 stop_reason 的最终事件。
                    self.merge_usage(&event["usage"]);
                }
            }
            "message_stop" => {
                self.terminal = true;
                if !self.finished {
                    return Err(error("message_stop 之前没有收到 stop_reason", None));
                }
            }
            "error" => {
                self.failed = true;
                parts.push(StreamPart::Error {
                    message: event["error"]["message"]
                        .as_str()
                        .unwrap_or("Messages 服务端错误")
                        .into(),
                    raw: Some(event.clone()),
                });
            }
            // ping 与未来新增事件按版本策略忽略，开启 Raw 时仍可观察。
            _ => {}
        }
        Ok(())
    }

    /// 把完整的非流式消息视为一次性到达的全部事件。
    pub(in crate::anthropic) fn complete(
        &mut self,
        message: &Value,
    ) -> Result<Vec<StreamPart>, ModelError> {
        if message["type"] == "error" {
            return Err(error(
                message["error"]["message"]
                    .as_str()
                    .unwrap_or("Messages 服务端错误"),
                Some(message.clone()),
            ));
        }
        let content = message["content"]
            .as_array()
            .ok_or_else(|| error("Messages 响应缺少 content 数组", Some(message.clone())))?;
        let stop_reason = message["stop_reason"]
            .as_str()
            .ok_or_else(|| error("Messages 响应缺少 stop_reason", Some(message.clone())))?;
        let mut parts = vec![StreamPart::ResponseMetadata(response_metadata(message))];
        for (index, block) in content.iter().enumerate() {
            let index = index as u64;
            self.start_block(index, block, &mut parts)?;
            self.stop_block(index, &mut parts)?;
        }
        self.finish(stop_reason, message, &message["usage"], &mut parts)?;
        self.terminal = true;
        Ok(parts)
    }

    /// `stop` 是携带 `stop_sequence` / `stop_details` 的容器：流式为 `message_delta.delta`，
    /// 非流式为完整消息。`stop_details` 给出拒绝类别与说明（如安全分类器触发），需透出供诊断。
    fn finish(
        &mut self,
        stop_reason: &str,
        stop: &Value,
        usage_delta: &Value,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        if self.finished {
            return Err(error("收到重复的 stop_reason", None));
        }
        // 厂商总会先关闭所有内容块；这里兜底补齐，避免丢失已完整到达的工具调用。
        let mut open: Vec<u64> = self
            .blocks
            .iter()
            .filter(|(_, block)| !block.ended)
            .map(|(index, _)| *index)
            .collect();
        open.sort_unstable();
        for index in open {
            self.stop_block(index, parts)?;
        }
        self.merge_usage(usage_delta);
        parts.push(StreamPart::Finish {
            usage: usage(self.usage.as_ref().unwrap_or(&Value::Null)),
            finish_reason: finish_reason(stop_reason),
            provider_metadata: metadata(json!({
                "stop_reason": stop_reason,
                "stop_sequence": stop["stop_sequence"],
                "stop_details": stop["stop_details"],
            })),
        });
        self.finished = true;
        Ok(())
    }

    fn merge_usage(&mut self, incoming: &Value) {
        let Some(fields) = incoming.as_object() else {
            return;
        };
        let target = self.usage.get_or_insert_with(|| json!({}));
        if let Some(existing) = target.as_object_mut() {
            for (key, value) in fields {
                if !value.is_null() {
                    existing.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

fn index(event: &Value) -> Result<u64, ModelError> {
    event["index"]
        .as_u64()
        .ok_or_else(|| error("内容块事件缺少 index", Some(event.clone())))
}
