//! Chat Completions 增量事件转换：解析器接入共用 SSE 驱动。
//!
//! 每个 `choice` 的推理、正文、拒绝与工具调用由 [`choice::ChoiceState`] 维护；
//! 本文件只处理分块级别的事务：`[DONE]`、错误分块、响应元数据、用量与收尾。

mod choice;

#[cfg(test)]
mod tests;

use bytes::Bytes;
use futures::Stream;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use zach_ai_core::{FinishReason, LanguageModelStream, StreamPart, Usage};

use super::response::{finish_reason, response_metadata, usage};
use crate::transport::{sse_stream, SseParser};
use choice::ChoiceState;

pub(super) struct ChatStreamParser {
    raw: bool,
    /// 按 `choice.index` 排序，收尾事件因此有确定的顺序。
    choices: BTreeMap<u64, ChoiceState>,
    /// 已收到的 `finish_reason`，等用量到达或 `[DONE]` 时再发 `Finish`。
    pending_finish: Option<FinishReason>,
    pending_usage: Option<Usage>,
    /// 响应 id / 模型 / 时间戳在所有分块里相同，只在首个分块透出一次。
    metadata_sent: bool,
    finished: bool,
    pub(super) terminal: bool,
    pub(super) failed: bool,
}

impl ChatStreamParser {
    pub(super) fn new(raw: bool) -> Self {
        Self {
            raw,
            choices: BTreeMap::new(),
            pending_finish: None,
            pending_usage: None,
            metadata_sent: false,
            finished: false,
            terminal: false,
            failed: false,
        }
    }

    pub(super) fn data(&mut self, data: &str) -> Vec<StreamPart> {
        if data == "[DONE]" {
            self.terminal = true;
            let mut parts = self.finish_pending();
            if !self.finished {
                self.failed = true;
                parts.push(StreamPart::Error {
                    message: "Chat Completions 在收到结束标记前没有 finish_reason".into(),
                    raw: None,
                });
            }
            return parts;
        }
        let event = match serde_json::from_str::<Value>(data) {
            Ok(event) => event,
            Err(error) => {
                self.failed = true;
                return vec![StreamPart::Error {
                    message: format!("Chat Completions SSE JSON 解析失败: {error}"),
                    raw: Some(json!(data)),
                }];
            }
        };
        let mut parts = Vec::new();
        if self.raw {
            parts.push(StreamPart::Raw {
                raw_value: event.clone(),
            });
        }
        // 部分代理会固定带上 `"error": null`，只有非空对象才是错误。
        if let Some(error) = event.get("error").filter(|error| !error.is_null()) {
            self.failed = true;
            parts.push(StreamPart::Error {
                message: error["message"]
                    .as_str()
                    .unwrap_or("Chat Completions 服务端错误")
                    .into(),
                raw: Some(error.clone()),
            });
            return parts;
        }
        if !self.metadata_sent && event["id"].as_str().is_some() {
            self.metadata_sent = true;
            parts.push(StreamPart::ResponseMetadata(response_metadata(&event)));
        }
        for choice in event["choices"].as_array().into_iter().flatten() {
            self.choice(choice, &mut parts);
        }
        // 用量通常在所有 choice 结束后单独一块到达，也可能与最后一个增量同块。
        if let Some(value) = event.get("usage").filter(|v| !v.is_null()) {
            self.pending_usage = Some(usage(value));
            parts.extend(self.finish_pending());
        }
        parts
    }

    fn choice(&mut self, choice: &Value, parts: &mut Vec<StreamPart>) {
        let index = choice["index"].as_u64().unwrap_or(0);
        self.choices
            .entry(index)
            .or_insert_with(|| ChoiceState::new(index))
            .delta(&choice["delta"], parts);
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.pending_finish = Some(finish_reason(Some(reason)));
        }
    }

    /// 收到 `finish_reason` 且用量已到（或流已 `[DONE]`）时关闭全部内容块并发出 `Finish`。
    fn finish_pending(&mut self) -> Vec<StreamPart> {
        if self.finished {
            return vec![];
        }
        let Some(reason) = self.pending_finish.clone() else {
            return vec![];
        };
        let mut parts = Vec::new();
        for choice in self.choices.values_mut() {
            choice.finish(&mut parts);
        }
        parts.push(StreamPart::Finish {
            usage: self.pending_usage.take().unwrap_or_default(),
            finish_reason: reason,
            provider_metadata: None,
        });
        self.finished = true;
        parts
    }
}

impl SseParser for ChatStreamParser {
    fn data(&mut self, data: &str) -> Vec<StreamPart> {
        ChatStreamParser::data(self, data)
    }

    fn terminal(&self) -> bool {
        self.terminal
    }

    fn failed(&self) -> bool {
        self.failed
    }
}

pub(super) fn chat_stream<S, E>(source: S, parser: ChatStreamParser) -> LanguageModelStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    sse_stream(super::LABEL, source, parser)
}
