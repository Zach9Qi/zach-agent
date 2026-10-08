//! 保存实际模型请求、完整响应与统一事件计数，供断言检查真实链路。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use zach_ai_core::{CallOptions, GenerateResult, Message, StreamPart, UnifiedFinishReason};

pub(super) type SharedTrace = Arc<Mutex<Trace>>;

#[derive(Default)]
pub(super) struct Trace {
    pub(super) calls: Vec<Call>,
}

pub(super) struct Call {
    pub(super) request: CallOptions,
    pub(super) result: Option<GenerateResult>,
    pub(super) events: HashMap<String, usize>,
}

pub(super) struct Outcome {
    pub(super) text: String,
    pub(super) finish_reason: UnifiedFinishReason,
    /// 仅本次新增的消息，不包含场景提供的初始历史。
    pub(super) messages: Vec<Message>,
    pub(super) steps: usize,
}

impl Trace {
    pub(super) fn request(&mut self, options: &CallOptions) -> usize {
        self.calls.push(Call {
            request: options.clone(),
            result: None,
            events: HashMap::new(),
        });
        self.calls.len() - 1
    }

    pub(super) fn event(&mut self, call: usize, part: &StreamPart) {
        // 名称直接沿用标准事件的序列化标签，避免维护另一份事件枚举。
        if let Ok(value) = serde_json::to_value(part) {
            if let Some(kind) = value["type"].as_str() {
                *self.calls[call].events.entry(kind.into()).or_default() += 1;
            }
        }
    }
}
