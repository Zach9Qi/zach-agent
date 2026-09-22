//! 流式分块累加器（将 StreamPart 流还原为 GenerateResult）
//!
//! 文本、推理和工具入参按**第一次出现**的位置占位，后续增量原地拼接。
//! 因此未发送 End、或 End 晚于其他内容到达时，最终顺序仍与流的起始顺序一致。

use std::collections::{HashMap, HashSet};

use crate::options::{ModelWarning, ProviderMetadata};
use crate::response::{
    FinishReason, GenerateResult, OutputContent, ResponseMetadata, UnifiedFinishReason, Usage,
};
use crate::stream::part::StreamPart;
use serde_json::Value;

/// 流式事件聚合器
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    /// 警告列表
    pub warnings: Vec<ModelWarning>,
    /// 响应元数据
    pub response: Option<ResponseMetadata>,
    /// 按首次出现顺序排列的实时内容。
    ///
    /// 只收到 Start、尚无增量的文本或推理段会暂时是空字符串，[`Self::finish`] 时剔除。
    pub content: Vec<OutputContent>,
    /// 当前已解析的结束原因。仅在收到 `Finish` 或 `Error` 后有值；二者都没有时由 `finish` 视为正常停止。
    pub finish_reason: Option<FinishReason>,
    /// Token 消耗统计
    pub usage: Option<Usage>,
    /// 最终厂商元数据
    pub provider_metadata: Option<ProviderMetadata>,

    text_index: HashMap<String, usize>,
    reasoning_index: HashMap<String, usize>,
    tool_index: HashMap<String, usize>,
    /// 已收到完整入参的工具调用。之后的增量片段不再拼接，避免和完整 `ToolCall` 重复。
    sealed_tools: HashSet<String>,
    explicit_finish: Option<FinishReason>,
    stream_error: Option<String>,
}

impl StreamAccumulator {
    /// 创建新的空聚合器
    pub fn new() -> Self {
        Self::default()
    }

    /// 接收并处理一个流式事件分块
    pub fn process(&mut self, part: StreamPart) {
        match part {
            StreamPart::StreamStart { warnings } => {
                self.warnings.extend(warnings);
            }
            StreamPart::ResponseMetadata(metadata) => {
                self.response = Some(metadata);
            }
            StreamPart::TextStart {
                id,
                provider_metadata,
            } => {
                self.ensure_text(&id, provider_metadata);
            }
            StreamPart::TextDelta {
                id,
                delta,
                provider_metadata,
            } => {
                let idx = self.ensure_text(&id, None);
                self.append_text(idx, &delta, provider_metadata);
            }
            StreamPart::TextEnd {
                id,
                provider_metadata,
            } => {
                self.finish_text(&id, provider_metadata);
            }
            StreamPart::ReasoningStart {
                id,
                provider_metadata,
            } => {
                self.ensure_reasoning(&id, provider_metadata);
            }
            StreamPart::ReasoningDelta {
                id,
                delta,
                provider_metadata,
            } => {
                let idx = self.ensure_reasoning(&id, None);
                self.append_reasoning(idx, &delta, provider_metadata);
            }
            StreamPart::ReasoningEnd {
                id,
                provider_metadata,
            } => {
                self.finish_reasoning(&id, provider_metadata);
            }
            StreamPart::ToolInputStart {
                id,
                tool_name,
                provider_executed,
                dynamic,
                provider_metadata,
                ..
            } => {
                self.begin_tool(
                    &id,
                    tool_name,
                    provider_executed,
                    dynamic,
                    provider_metadata,
                );
            }
            StreamPart::ToolInputDelta {
                id,
                delta,
                provider_metadata,
            } => {
                self.append_tool_input(&id, &delta, provider_metadata);
            }
            StreamPart::ToolInputEnd {
                id,
                provider_metadata,
            } => {
                self.finish_tool_input(&id, provider_metadata);
            }
            StreamPart::ToolCall {
                tool_call_id,
                tool_name,
                input,
                provider_executed,
                dynamic,
                provider_metadata,
            } => {
                self.apply_tool_call(
                    tool_call_id,
                    tool_name,
                    input,
                    provider_executed,
                    dynamic,
                    provider_metadata,
                );
            }
            StreamPart::ToolResult {
                tool_call_id,
                tool_name,
                result,
                is_error,
                preliminary,
                dynamic,
                provider_metadata,
            } => {
                self.content.push(OutputContent::ToolResult {
                    tool_call_id,
                    tool_name,
                    result,
                    is_error,
                    preliminary,
                    dynamic,
                    provider_metadata,
                });
            }
            StreamPart::ToolApprovalRequest {
                approval_id,
                tool_call_id,
                provider_metadata,
            } => {
                self.content.push(OutputContent::ToolApprovalRequest {
                    approval_id,
                    tool_call_id,
                    provider_metadata,
                });
            }
            StreamPart::File {
                media_type,
                data,
                provider_metadata,
            } => {
                self.content.push(OutputContent::File {
                    media_type,
                    data,
                    provider_metadata,
                });
            }
            StreamPart::ReasoningFile {
                media_type,
                data,
                provider_metadata,
            } => {
                self.content.push(OutputContent::ReasoningFile {
                    media_type,
                    data,
                    provider_metadata,
                });
            }
            StreamPart::Source(source) => {
                self.content.push(OutputContent::Source(source));
            }
            StreamPart::Custom {
                kind,
                data,
                provider_metadata,
            } => {
                self.content.push(OutputContent::Custom {
                    kind,
                    data,
                    provider_metadata,
                });
            }
            StreamPart::Finish {
                usage,
                finish_reason,
                provider_metadata,
            } => {
                self.usage = Some(usage);
                self.explicit_finish = Some(finish_reason);
                self.publish_finish_reason();
                if provider_metadata.is_some() {
                    self.provider_metadata = provider_metadata;
                }
            }
            StreamPart::Error { message, raw } => {
                self.stream_error = Some(describe_stream_error(message, raw));
                self.publish_finish_reason();
            }
            StreamPart::Raw { .. } => {}
        }
    }

    /// 结束聚合，生成最终的 [`GenerateResult`]
    pub fn finish(mut self) -> GenerateResult {
        self.content.retain(keep_in_final_content);
        let finish_reason = self.resolved_finish_reason();
        GenerateResult {
            content: self.content,
            finish_reason,
            usage: self.usage.unwrap_or_default(),
            warnings: self.warnings,
            provider_metadata: self.provider_metadata,
            response: self.response,
            request_body: None,
            response_headers: None,
        }
    }

    fn ensure_text(&mut self, id: &str, metadata: Option<ProviderMetadata>) -> usize {
        if let Some(&idx) = self.text_index.get(id) {
            if let OutputContent::Text {
                provider_metadata, ..
            } = &mut self.content[idx]
            {
                merge_metadata(provider_metadata, metadata);
            }
            idx
        } else {
            let idx = self.content.len();
            self.content.push(OutputContent::Text {
                text: String::new(),
                provider_metadata: metadata,
            });
            self.text_index.insert(id.to_string(), idx);
            idx
        }
    }

    fn append_text(&mut self, idx: usize, delta: &str, metadata: Option<ProviderMetadata>) {
        if let OutputContent::Text {
            text,
            provider_metadata,
        } = &mut self.content[idx]
        {
            text.push_str(delta);
            merge_metadata(provider_metadata, metadata);
        }
    }

    fn finish_text(&mut self, id: &str, metadata: Option<ProviderMetadata>) {
        let Some(&idx) = self.text_index.get(id) else {
            return;
        };
        if let OutputContent::Text {
            provider_metadata, ..
        } = &mut self.content[idx]
        {
            merge_metadata(provider_metadata, metadata);
        }
    }

    fn ensure_reasoning(&mut self, id: &str, metadata: Option<ProviderMetadata>) -> usize {
        if let Some(&idx) = self.reasoning_index.get(id) {
            if let OutputContent::Reasoning {
                provider_metadata, ..
            } = &mut self.content[idx]
            {
                merge_metadata(provider_metadata, metadata);
            }
            idx
        } else {
            let idx = self.content.len();
            self.content.push(OutputContent::Reasoning {
                text: String::new(),
                provider_metadata: metadata,
            });
            self.reasoning_index.insert(id.to_string(), idx);
            idx
        }
    }

    fn append_reasoning(&mut self, idx: usize, delta: &str, metadata: Option<ProviderMetadata>) {
        if let OutputContent::Reasoning {
            text,
            provider_metadata,
        } = &mut self.content[idx]
        {
            text.push_str(delta);
            merge_metadata(provider_metadata, metadata);
        }
    }

    fn finish_reasoning(&mut self, id: &str, metadata: Option<ProviderMetadata>) {
        let Some(&idx) = self.reasoning_index.get(id) else {
            return;
        };
        if let OutputContent::Reasoning {
            provider_metadata, ..
        } = &mut self.content[idx]
        {
            merge_metadata(provider_metadata, metadata);
        }
    }

    fn begin_tool(
        &mut self,
        id: &str,
        tool_name: String,
        provider_executed: bool,
        dynamic: bool,
        metadata: Option<ProviderMetadata>,
    ) {
        let idx = self.ensure_tool(id);
        let sealed = self.sealed_tools.contains(id);
        if let OutputContent::ToolCall {
            tool_name: name,
            provider_executed: executed,
            dynamic: is_dynamic,
            provider_metadata,
            ..
        } = &mut self.content[idx]
        {
            if !tool_name.is_empty() && (name.is_empty() || !sealed) {
                *name = tool_name;
            }
            if !sealed {
                *executed = provider_executed;
                *is_dynamic = dynamic;
            }
            merge_metadata(provider_metadata, metadata);
        }
    }

    fn append_tool_input(&mut self, id: &str, delta: &str, metadata: Option<ProviderMetadata>) {
        let idx = self.ensure_tool(id);
        let sealed = self.sealed_tools.contains(id);
        if let OutputContent::ToolCall {
            input,
            provider_metadata,
            ..
        } = &mut self.content[idx]
        {
            if !sealed {
                input.push_str(delta);
            }
            merge_metadata(provider_metadata, metadata);
        }
    }

    fn finish_tool_input(&mut self, id: &str, metadata: Option<ProviderMetadata>) {
        let Some(&idx) = self.tool_index.get(id) else {
            return;
        };
        if let OutputContent::ToolCall {
            provider_metadata, ..
        } = &mut self.content[idx]
        {
            merge_metadata(provider_metadata, metadata);
        }
    }

    fn apply_tool_call(
        &mut self,
        id: String,
        tool_name: String,
        input: String,
        provider_executed: bool,
        dynamic: bool,
        metadata: Option<ProviderMetadata>,
    ) {
        let seal = !input.is_empty();
        let idx = self.ensure_tool(&id);
        if let OutputContent::ToolCall {
            tool_name: name,
            input: slot_input,
            provider_executed: executed,
            dynamic: is_dynamic,
            provider_metadata,
            ..
        } = &mut self.content[idx]
        {
            if !tool_name.is_empty() {
                *name = tool_name;
            }
            if seal {
                *slot_input = input;
            }
            *executed = provider_executed;
            *is_dynamic = dynamic;
            merge_metadata(provider_metadata, metadata);
        }
        if seal {
            self.sealed_tools.insert(id);
        }
    }

    fn ensure_tool(&mut self, id: &str) -> usize {
        if let Some(&idx) = self.tool_index.get(id) {
            idx
        } else {
            let idx = self.content.len();
            self.content.push(OutputContent::ToolCall {
                tool_call_id: id.to_string(),
                tool_name: String::new(),
                input: String::new(),
                provider_executed: false,
                dynamic: false,
                provider_metadata: None,
            });
            self.tool_index.insert(id.to_string(), idx);
            idx
        }
    }

    fn publish_finish_reason(&mut self) {
        self.finish_reason = Some(self.resolved_finish_reason());
    }

    fn resolved_finish_reason(&self) -> FinishReason {
        match (&self.explicit_finish, &self.stream_error) {
            (Some(reason), Some(err)) if reason.unified == UnifiedFinishReason::Stop => {
                FinishReason {
                    unified: UnifiedFinishReason::Error,
                    raw: Some(err.clone()),
                }
            }
            (Some(reason), _) => reason.clone(),
            (None, Some(err)) => FinishReason {
                unified: UnifiedFinishReason::Error,
                raw: Some(err.clone()),
            },
            (None, None) => FinishReason {
                unified: UnifiedFinishReason::Stop,
                raw: None,
            },
        }
    }
}

fn merge_metadata(slot: &mut Option<ProviderMetadata>, incoming: Option<ProviderMetadata>) {
    if incoming.is_some() {
        *slot = incoming;
    }
}

fn keep_in_final_content(part: &OutputContent) -> bool {
    match part {
        OutputContent::Text { text, .. } | OutputContent::Reasoning { text, .. } => {
            !text.is_empty()
        }
        _ => true,
    }
}

fn describe_stream_error(message: String, raw: Option<Value>) -> String {
    if !message.is_empty() {
        message
    } else if let Some(raw) = raw {
        raw.to_string()
    } else {
        "未知流式错误".to_string()
    }
}
