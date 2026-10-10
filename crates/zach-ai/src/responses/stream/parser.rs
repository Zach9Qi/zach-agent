//! Responses 语义事件状态机，区分输出项 ID 与函数调用 ID。

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use zach_ai_core::{ModelError, StreamPart};

use super::super::response::{finish, response_metadata, string};
use super::items::SUMMARY_SEPARATOR;
use super::text::Block;

pub(in crate::responses) struct ResponsesStreamParser {
    pub(super) items: HashMap<String, Value>,
    pub(super) completed: HashSet<String>,
    pub(super) calls_done: HashMap<String, String>,
    pub(super) blocks: HashMap<String, Block>,
    pub(super) sources: HashSet<String>,
    /// 每个推理项最近一次增量的 `summary_index`，用于在换段时补分隔。
    summary_index: HashMap<String, u64>,
    pub(super) terminal: bool,
    pub(super) failed: bool,
    raw: bool,
}

impl ResponsesStreamParser {
    pub(in crate::responses) fn new(raw: bool) -> Self {
        Self {
            items: HashMap::new(),
            completed: HashSet::new(),
            calls_done: HashMap::new(),
            blocks: HashMap::new(),
            sources: HashSet::new(),
            summary_index: HashMap::new(),
            terminal: false,
            failed: false,
            raw,
        }
    }

    pub(super) fn data(&mut self, data: &str) -> Vec<StreamPart> {
        if data == "[DONE]" {
            // Responses 以语义终止事件收尾；兼容代理的标记不构成成功证明。
            return vec![];
        }
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
                    message: format!("Responses SSE JSON 解析失败: {error}"),
                    raw: Some(json!(data)),
                });
            }
        }
        parts
    }

    fn event(&mut self, event: &Value, parts: &mut Vec<StreamPart>) -> Result<(), ModelError> {
        match string(event, "type")? {
            "response.created" | "response.in_progress" => parts.push(
                StreamPart::ResponseMetadata(response_metadata(&event["response"])),
            ),
            "response.output_item.added" => self.start_item(&event["item"], parts)?,
            "response.output_item.done" => self.finish_item(&event["item"], parts)?,
            "response.content_part.added" => {
                let id = text_id(event)?;
                let refusal = event["part"]["type"] == "refusal";
                self.start_block(&id, false, self.text_metadata(event, refusal), parts);
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let id = text_id(event)?;
                self.delta(
                    &id,
                    string(event, "delta")?,
                    false,
                    self.text_metadata(event, event["type"] == "response.refusal.delta"),
                    parts,
                )?;
            }
            "response.output_text.done" | "response.refusal.done" => {
                let id = text_id(event)?;
                let refusal = event["type"] == "response.refusal.done";
                self.reconcile(
                    &id,
                    string(event, if refusal { "refusal" } else { "text" })?,
                    false,
                    self.text_metadata(event, refusal),
                    parts,
                )?;
            }
            "response.content_part.done" => {
                let item_id = string(event, "item_id")?;
                let index = content_index(event)?;
                let item = self
                    .items
                    .get(item_id)
                    .cloned()
                    .unwrap_or_else(|| json!({"id": item_id}));
                self.finish_content(&item, index, &event["part"], parts)?;
            }
            "response.reasoning_summary_text.delta" => {
                let id = string(event, "item_id")?;
                let index = event["summary_index"].as_u64().unwrap_or(0);
                let delta = format!(
                    "{}{}",
                    self.summary_separator(id, index),
                    string(event, "delta")?
                );
                self.delta(id, &delta, true, None, parts)?;
            }
            // gpt-oss 一类开源服务没有摘要，直接下发原始推理正文。
            "response.reasoning_text.delta" => {
                let id = string(event, "item_id")?;
                self.delta(id, string(event, "delta")?, true, None, parts)?;
            }
            "response.function_call_arguments.delta" => {
                let id = string(event, "item_id")?;
                let item = self.items.get(id).ok_or_else(|| {
                    ModelError::provider_error(
                        "openai",
                        "工具参数增量缺少对应输出项",
                        Some(event.clone()),
                    )
                })?;
                if !self.calls_done.contains_key(id) {
                    parts.push(StreamPart::ToolInputDelta {
                        id: string(item, "call_id")?.into(),
                        delta: string(event, "delta")?.into(),
                        provider_metadata: None,
                    });
                }
            }
            "response.function_call_arguments.done" => {
                let id = string(event, "item_id")?;
                let mut item = self.items.get(id).cloned().ok_or_else(|| {
                    ModelError::provider_error(
                        "openai",
                        "工具参数完成事件缺少对应输出项",
                        Some(event.clone()),
                    )
                })?;
                item["arguments"] = json!(string(event, "arguments")?);
                self.finish_call(&item, parts)?;
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                let status = string(&event["response"], "status")?;
                if event["type"] != format!("response.{status}") {
                    return Err(ModelError::provider_error(
                        "openai",
                        "终止事件与响应状态不一致",
                        Some(event.clone()),
                    ));
                }
                parts.extend(self.complete(&event["response"])?);
                self.terminal = true;
            }
            "error" => {
                self.failed = true;
                parts.push(StreamPart::Error {
                    message: event["message"]
                        .as_str()
                        .unwrap_or("Responses 服务端错误")
                        .into(),
                    raw: Some(event.clone()),
                });
            }
            // 摘要与正文的完成快照由 output_item.done 补齐，其他进度事件只按需透出 Raw。
            _ => {}
        }
        Ok(())
    }

    /// 推理摘要换到更大的 `summary_index` 时补一个分隔，与完成快照把多段按空行拼接一致。
    fn summary_separator(&mut self, item_id: &str, index: u64) -> &'static str {
        match self.summary_index.insert(item_id.to_owned(), index) {
            Some(previous) if index > previous => SUMMARY_SEPARATOR,
            _ => "",
        }
    }

    pub(in crate::responses) fn complete(
        &mut self,
        response: &Value,
    ) -> Result<Vec<StreamPart>, ModelError> {
        let terminal = finish(response)?;
        let mut parts = vec![StreamPart::ResponseMetadata(response_metadata(response))];
        if matches!(response["status"].as_str(), Some("failed" | "cancelled")) {
            self.failed = true;
            parts.push(StreamPart::Error {
                message: response
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Responses 生成失败")
                    .into(),
                raw: Some(response.clone()),
            });
        } else {
            for item in response["output"].as_array().expect("finish 已校验 output") {
                // 单个输出项解析失败不能吞掉终止事件：降级为 Error 并继续
                // 处理其余项，保证末尾的 Finish（用量与停止原因）始终发出。
                if let Err(error) = self.finish_item(item, &mut parts) {
                    self.failed = true;
                    parts.push(StreamPart::Error {
                        message: error.to_string(),
                        raw: Some(item.clone()),
                    });
                }
            }
        }
        parts.push(terminal);
        Ok(parts)
    }
}

fn content_index(event: &Value) -> Result<u64, ModelError> {
    event["content_index"].as_u64().ok_or_else(|| {
        ModelError::provider_error("openai", "文本事件缺少 content_index", Some(event.clone()))
    })
}

fn text_id(event: &Value) -> Result<String, ModelError> {
    Ok(format!(
        "{}:{}",
        string(event, "item_id")?,
        content_index(event)?
    ))
}
