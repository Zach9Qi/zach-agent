//! 单个 `choice` 的内容块生命周期：推理、正文、拒绝与工具调用。
//!
//! 块标识与非流式响应保持一致：正文 `choice:{index}`，推理与拒绝分别加
//! `/reasoning`、`/refusal` 后缀，便于调用方在两种模式下用同一套 id 定位。

use serde_json::{json, Value};
use std::collections::BTreeMap;
use zach_ai_core::StreamPart;

use super::super::response::{metadata, reasoning_text};

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
    /// 是否已发出 `Error`（如工具调用始终没有 id），由解析器汇总为本轮失败。
    pub(super) failed: bool,
}

impl ChoiceState {
    pub(super) fn new(index: u64) -> Self {
        Self {
            index,
            reasoning: Block::Pending,
            text: Block::Pending,
            refusal: Block::Pending,
            tools: BTreeMap::new(),
            failed: false,
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

    /// 处理一个增量。块一律在首个**非空**增量时才开始（LiteLLM、vLLM 等网关惯用
    /// 空字符串占位，照单全收会产生空块与误判的生命周期）；已结束的块不再接受
    /// 增量——End 之后不能再有 Delta，迟到内容只能丢弃。
    pub(super) fn delta(&mut self, delta: &Value, parts: &mut Vec<StreamPart>) {
        if let Some(reasoning) = reasoning_text(delta).filter(|s| !s.is_empty()) {
            if self.reasoning == Block::Pending {
                self.reasoning = Block::Open;
                parts.push(StreamPart::ReasoningStart {
                    id: self.reasoning_id(),
                    provider_metadata: None,
                });
            }
            if self.reasoning == Block::Open {
                parts.push(StreamPart::ReasoningDelta {
                    id: self.reasoning_id(),
                    delta: reasoning.into(),
                    provider_metadata: None,
                });
            }
        }
        // 首个非空的正文、拒绝或工具调用增量标志思考阶段结束；不等流收尾再关闭，
        // 否则消费方要等整段正文输出完才看到"推理结束"。空 `content` 占位不算正文开始。
        let answer_begun = delta["content"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
            || delta["refusal"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
            || delta["tool_calls"]
                .as_array()
                .is_some_and(|calls| !calls.is_empty());
        if answer_begun {
            self.close_reasoning(parts);
        }
        if let Some(text) = delta["content"].as_str().filter(|s| !s.is_empty()) {
            if self.text == Block::Pending {
                self.text = Block::Open;
                parts.push(StreamPart::TextStart {
                    id: self.text_id(),
                    provider_metadata: metadata(json!({"refusal": false})),
                });
            }
            if self.text == Block::Open {
                parts.push(StreamPart::TextDelta {
                    id: self.text_id(),
                    delta: text.into(),
                    provider_metadata: None,
                });
            }
        }
        if let Some(refusal) = delta["refusal"].as_str().filter(|s| !s.is_empty()) {
            if self.refusal == Block::Pending {
                self.refusal = Block::Open;
                parts.push(StreamPart::TextStart {
                    id: self.refusal_id(),
                    provider_metadata: metadata(json!({"refusal": true})),
                });
            }
            if self.refusal == Block::Open {
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
    /// id 到达之前的参数只缓存不透出，避免泄漏 id 为空的事件。
    fn tool_delta(&mut self, call: &Value, parts: &mut Vec<StreamPart>) {
        let tool_index = call["index"].as_u64().unwrap_or(0);
        let incoming_id = call["id"].as_str().filter(|id| !id.is_empty());
        // 部分兼容端点对先后发起的多次调用复用同一个 index（甚至不带 index），
        // 只能靠 id 变化识别新调用：先把上一个按完整调用发出，再另起状态。
        if let Some(previous) = self.tools.get_mut(&tool_index) {
            if incoming_id.is_some_and(|id| !previous.id.is_empty() && previous.id != id) {
                if finish_tool(previous, parts) {
                    self.failed = true;
                }
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
        if entry.ended {
            // 已完整发出的调用不再接受任何字段（防御收尾后的迟到增量）。
            return;
        }
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
            // id 晚于参数到达：把此前缓存的参数作为首个增量一次补发。
            if !entry.arguments.is_empty() {
                parts.push(StreamPart::ToolInputDelta {
                    id: entry.id.clone(),
                    delta: entry.arguments.clone(),
                    provider_metadata: None,
                });
            }
        }
        if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
            entry.arguments.push_str(arguments);
            if entry.started && !arguments.is_empty() {
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

    /// 收尾：推理、正文、拒绝依次结束，工具调用按编号发出完整调用。
    /// 未曾开始的块也一并置为 Closed，之后的迟到增量不会再开新块。
    pub(super) fn finish(&mut self, parts: &mut Vec<StreamPart>) {
        self.close_reasoning(parts);
        self.reasoning = Block::Closed;
        if self.text == Block::Open {
            parts.push(StreamPart::TextEnd {
                id: self.text_id(),
                provider_metadata: None,
            });
        }
        self.text = Block::Closed;
        if self.refusal == Block::Open {
            parts.push(StreamPart::TextEnd {
                id: self.refusal_id(),
                provider_metadata: None,
            });
        }
        self.refusal = Block::Closed;
        for state in self.tools.values_mut() {
            if finish_tool(state, parts) {
                self.failed = true;
            }
        }
    }
}

/// 结束一个工具调用：发出入参结束与完整调用。
///
/// 始终没有 id 的调用无法与执行结果配对，不能当作正常调用发出；但 finish_reason 多半仍是
/// `tool_calls`，静默丢弃会让上层以为模型"什么都没调用"，因此以 `Error` 报告，返回 `true`。
/// 既无 id 也无名称和参数的空壳（仅 `index` 的占位增量）直接忽略。
fn finish_tool(state: &mut ToolState, parts: &mut Vec<StreamPart>) -> bool {
    if state.ended {
        return false;
    }
    state.ended = true;
    if state.id.is_empty() {
        if state.name.is_empty() && state.arguments.is_empty() {
            return false;
        }
        parts.push(StreamPart::Error {
            message: "Chat Completions 工具调用缺少 id，无法与执行结果配对".into(),
            raw: Some(json!({"name": state.name, "arguments": state.arguments})),
        });
        return true;
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
    false
}
