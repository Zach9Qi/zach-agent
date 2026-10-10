//! Chat Completions SSE 解码和增量事件转换。

mod sse;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use zach_ai_core::{LanguageModelStream, ModelError, StreamPart};

use super::response::{finish_reason, metadata, response_metadata, usage};
use sse::SseDecoder;

struct ToolState {
    id: String,
    name: String,
    arguments: String,
    started: bool,
    ended: bool,
}

pub(super) struct ChatStreamParser {
    texts: HashSet<String>,
    text_buffers: HashMap<String, String>,
    reasonings: HashSet<String>,
    tools: HashMap<(u64, u64), ToolState>,
    raw: bool,
    pending_finish: Option<zach_ai_core::FinishReason>,
    pending_usage: Option<zach_ai_core::Usage>,
    finished: bool,
    pub(super) terminal: bool,
    pub(super) failed: bool,
}

impl ChatStreamParser {
    pub(super) fn new(raw: bool) -> Self {
        Self {
            texts: HashSet::new(),
            text_buffers: HashMap::new(),
            reasonings: HashSet::new(),
            tools: HashMap::new(),
            raw,
            pending_finish: None,
            pending_usage: None,
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
        if let Some(error) = event.get("error") {
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
        if let Some(id) = event["id"].as_str() {
            parts.push(StreamPart::ResponseMetadata(response_metadata(&event)));
            let _ = id;
        }
        let value = event.get("usage").filter(|v| !v.is_null()).map(usage);
        if let Some(value) = value {
            self.pending_usage = Some(value);
            parts.extend(self.finish_pending());
        }
        if let Some(choices) = event["choices"].as_array() {
            for choice in choices {
                self.choice(choice, &mut parts);
            }
        }
        parts
    }

    fn choice(&mut self, choice: &Value, parts: &mut Vec<StreamPart>) {
        let index = choice["index"].as_u64().unwrap_or(0);
        let delta = &choice["delta"];
        // DeepSeek、Qwen 等兼容端点通过 reasoning_content 下发思考链增量。
        if let Some(reasoning) = delta["reasoning_content"].as_str() {
            let id = format!("choice:{index}/reasoning");
            if self.reasonings.insert(id.clone()) {
                parts.push(StreamPart::ReasoningStart {
                    id: id.clone(),
                    provider_metadata: None,
                });
            }
            if !reasoning.is_empty() {
                parts.push(StreamPart::ReasoningDelta {
                    id,
                    delta: reasoning.into(),
                    provider_metadata: None,
                });
            }
        }
        if let Some(text) = delta["content"].as_str() {
            let id = format!("choice:{index}");
            if self.texts.insert(id.clone()) {
                parts.push(StreamPart::TextStart {
                    id: id.clone(),
                    provider_metadata: metadata(json!({"refusal": false})),
                });
                self.text_buffers.insert(id.clone(), String::new());
            }
            if !text.is_empty() {
                self.text_buffers
                    .entry(id.clone())
                    .or_default()
                    .push_str(text);
                parts.push(StreamPart::TextDelta {
                    id,
                    delta: text.into(),
                    provider_metadata: None,
                });
            }
        }
        if let Some(refusal) = delta["refusal"].as_str() {
            let id = format!("choice:{index}/refusal");
            if self.texts.insert(id.clone()) {
                parts.push(StreamPart::TextStart {
                    id: id.clone(),
                    provider_metadata: metadata(json!({"refusal": true})),
                });
                self.text_buffers.insert(id.clone(), String::new());
            }
            if !refusal.is_empty() {
                self.text_buffers
                    .entry(id.clone())
                    .or_default()
                    .push_str(refusal);
                parts.push(StreamPart::TextDelta {
                    id,
                    delta: refusal.into(),
                    provider_metadata: None,
                });
            }
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for call in calls {
                let tool_index = call["index"].as_u64().unwrap_or(0);
                let entry = self
                    .tools
                    .entry((index, tool_index))
                    .or_insert_with(|| ToolState {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                        started: false,
                        ended: false,
                    });
                if let Some(id) = call["id"].as_str() {
                    entry.id = id.into();
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    entry.name = name.into();
                }
                if !entry.id.is_empty() && !entry.ended && !entry.started {
                    parts.push(StreamPart::ToolInputStart {
                        id: entry.id.clone(),
                        tool_name: entry.name.clone(),
                        provider_executed: false,
                        dynamic: false,
                        title: None,
                        provider_metadata: None,
                    });
                    entry.started = true;
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                {
                    entry.arguments.push_str(arguments);
                    if !arguments.is_empty() {
                        parts.push(StreamPart::ToolInputDelta {
                            id: entry.id.clone(),
                            delta: arguments.into(),
                            provider_metadata: None,
                        });
                    }
                }
            }
        }
        if choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .is_some()
        {
            self.pending_finish = Some(finish_reason(choice["finish_reason"].as_str()));
        }
    }

    fn finish_pending(&mut self) -> Vec<StreamPart> {
        if self.finished {
            return vec![];
        }
        let Some(reason) = self.pending_finish.clone() else {
            return vec![];
        };
        let mut parts = Vec::new();
        for id in self.reasonings.clone() {
            parts.push(StreamPart::ReasoningEnd {
                id,
                provider_metadata: None,
            });
        }
        for id in self.texts.clone() {
            parts.push(StreamPart::TextEnd {
                id,
                provider_metadata: None,
            });
        }
        for state in self.tools.values_mut() {
            if !state.ended {
                if !state.id.is_empty() {
                    parts.push(StreamPart::ToolInputEnd {
                        id: state.id.clone(),
                        provider_metadata: None,
                    });
                    parts.push(StreamPart::ToolCall {
                        tool_call_id: state.id.clone(),
                        tool_name: state.name.clone(),
                        input: state.arguments.clone(),
                        provider_executed: false,
                        dynamic: false,
                        provider_metadata: None,
                    });
                }
                state.ended = true;
            }
        }
        parts.push(StreamPart::Finish {
            usage: self.pending_usage.clone().unwrap_or_default(),
            finish_reason: reason,
            provider_metadata: None,
        });
        self.finished = true;
        parts
    }
}

struct State<S> {
    source: Pin<Box<S>>,
    decoder: SseDecoder,
    parser: ChatStreamParser,
    queue: VecDeque<Result<StreamPart, ModelError>>,
    ended: bool,
}

pub(super) fn chat_stream<S, E>(source: S, parser: ChatStreamParser) -> LanguageModelStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let state = State {
        source: Box::pin(source),
        decoder: SseDecoder::default(),
        parser,
        queue: VecDeque::from([Ok(StreamPart::StreamStart { warnings: vec![] })]),
        ended: false,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(part) = state.queue.pop_front() {
                return Some((part, state));
            }
            if state.ended {
                return None;
            }
            match state.source.next().await {
                Some(Ok(bytes)) => match state.decoder.push(&bytes) {
                    Ok(frames) => {
                        for frame in frames {
                            state
                                .queue
                                .extend(state.parser.data(&frame).into_iter().map(Ok));
                            if state.parser.terminal {
                                state.ended = true;
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        state.queue.push_back(Err(error));
                        state.ended = true;
                    }
                },
                Some(Err(error)) => {
                    state.queue.push_back(Err(ModelError::stream_error(
                        "读取 Chat Completions SSE 字节流失败",
                        error,
                    )));
                    state.ended = true;
                }
                None => {
                    state.ended = true;
                    if !state.parser.terminal {
                        state.queue.push_back(Err(ModelError::StreamError {
                            message: "Chat Completions SSE 在 [DONE] 前结束".into(),
                            source: None,
                        }));
                    }
                }
            }
        }
    }))
}
