//! 单个 `choice` 的内容块生命周期：推理、正文、拒绝与工具调用。
//!
//! 块标识与非流式响应保持一致：正文 `choice:{index}`，推理与拒绝分别加
//! `/reasoning`、`/refusal` 后缀，便于调用方在两种模式下用同一套 id 定位。

use serde_json::{json, Value};
use std::collections::BTreeMap;
use zach_ai_core::StreamPart;

use super::super::response::metadata;

/// 文本类块的生命周期。
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Block {
    #[default]
    Pending,
    Open,
    Closed,
}

struct ToolState {
    id: String,
    name: String,
    arguments: String,
    started: bool,
    ended: bool,
}

pub(super) struct ChoiceState {
    index: u64,
    reasoning: Block,
    text: Block,
    refusal: Block,
    /// 按 `tool_calls[].index` 排序，并行调用的收尾顺序与厂商编号一致。
    tools: BTreeMap<u64, ToolState>,
}

impl ChoiceState {
    pub(super) fn new(index: u64) -> Self {
        Self {
            index,
            reasoning: Block::Pending,
            text: Block::Pending,
            refusal: Block::Pending,
            tools: BTreeMap::new(),
        }
    }

    fn text_id(&self) -> String {
        format!("choice:{}", self.index)
    }

    fn reasoning_id(&self) -> String {
        format!("choice:{}/reasoning", self.index)
    }

    fn refusal_id(&self) -> String {
        format!("choice:{}/refusal", self.index)
    }

    pub(super) fn delta(&mut self, delta: &Value, parts: &mut Vec<StreamPart>) {
        // DeepSeek、Qwen 等兼容端点通过 reasoning_content 下发思考链增量。
        if let Some(reasoning) = delta["reasoning_content"].as_str() {
            if self.reasoning == Block::Pending {
                self.reasoning = Block::Open;
                parts.push(StreamPart::ReasoningStart {
                    id: self.reasoning_id(),
                    provider_metadata: None,
                });
            }
            if !reasoning.is_empty() {
                parts.push(StreamPart::ReasoningDelta {
                    id: self.reasoning_id(),
                    delta: reasoning.into(),
                    provider_metadata: None,
                });
            }
        }
        // 正文、拒绝或工具调用一旦开始，思考阶段即告结束；不等到流收尾再关闭，
        // 否则消费方会在整段正文输出完之后才看到"推理结束"。
        if delta["content"].is_string()
            || delta["refusal"].is_string()
            || delta["tool_calls"].is_array()
        {
            self.close_reasoning(parts);
        }
        if let Some(text) = delta["content"].as_str() {
            if self.text == Block::Pending {
                self.text = Block::Open;
                parts.push(StreamPart::TextStart {
                    id: self.text_id(),
                    provider_metadata: metadata(json!({"refusal": false})),
                });
            }
            if !text.is_empty() {
                parts.push(StreamPart::TextDelta {
                    id: self.text_id(),
                    delta: text.into(),
                    provider_metadata: None,
                });
            }
        }
        if let Some(refusal) = delta["refusal"].as_str() {
            if self.refusal == Block::Pending {
                self.refusal = Block::Open;
                parts.push(StreamPart::TextStart {
                    id: self.refusal_id(),
                    provider_metadata: metadata(json!({"refusal": true})),
                });
            }
            if !refusal.is_empty() {
                parts.push(StreamPart::TextDelta {
                    id: self.refusal_id(),
                    delta: refusal.into(),
                    provider_metadata: None,
                });
            }
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            self.tool_delta(call, parts);
        }
    }

    /// 工具调用按 `index` 累积：`id` 与函数名通常只在首个增量出现，参数逐块拼接。
    fn tool_delta(&mut self, call: &Value, parts: &mut Vec<StreamPart>) {
        let tool_index = call["index"].as_u64().unwrap_or(0);
        let incoming_id = call["id"].as_str().filter(|id| !id.is_empty());
        // 部分兼容端点对先后发起的多次调用复用同一个 index（甚至不带 index），
        // 只能靠 id 变化识别新调用：先把上一个按完整调用发出，再另起状态。
        if let Some(previous) = self.tools.get_mut(&tool_index) {
            if incoming_id.is_some_and(|id| !previous.id.is_empty() && previous.id != id) {
                finish_tool(previous, parts);
                self.tools.remove(&tool_index);
            }
        }
        let entry = self.tools.entry(tool_index).or_insert_with(|| ToolState {
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
            started: false,
            ended: false,
        });
        if let Some(id) = incoming_id {
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
        if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
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

    fn close_reasoning(&mut self, parts: &mut Vec<StreamPart>) {
        if self.reasoning == Block::Open {
            self.reasoning = Block::Closed;
            parts.push(StreamPart::ReasoningEnd {
                id: self.reasoning_id(),
                provider_metadata: None,
            });
        }
    }

    /// 关闭仍打开的块：推理、正文、拒绝依次结束，工具调用按编号发出完整调用。
    pub(super) fn finish(&mut self, parts: &mut Vec<StreamPart>) {
        self.close_reasoning(parts);
        if self.text == Block::Open {
            self.text = Block::Closed;
            parts.push(StreamPart::TextEnd {
                id: self.text_id(),
                provider_metadata: None,
            });
        }
        if self.refusal == Block::Open {
            self.refusal = Block::Closed;
            parts.push(StreamPart::TextEnd {
                id: self.refusal_id(),
                provider_metadata: None,
            });
        }
        for state in self.tools.values_mut() {
            finish_tool(state, parts);
        }
    }
}

/// 结束一个工具调用：发出入参结束与完整调用；没有 id 的残缺调用无法配对结果，直接丢弃。
fn finish_tool(state: &mut ToolState, parts: &mut Vec<StreamPart>) {
    if state.ended {
        return;
    }
    state.ended = true;
    if state.id.is_empty() {
        return;
    }
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
