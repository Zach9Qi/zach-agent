//! 流式分块累加器（将 StreamPart 流还原为 GenerateResult）

use std::collections::HashMap;
use crate::options::{ModelWarning, ProviderMetadata};
use crate::response::{
    FinishReason, GenerateResult, OutputContent, ResponseMetadata, UnifiedFinishReason, Usage,
};
use crate::stream::part::StreamPart;

/// 工具入参累积状态
#[derive(Debug, Default)]
struct ToolInputAccumulator {
    id: String,
    tool_name: String,
    input_buffer: String,
    provider_executed: bool,
    dynamic: bool,
    provider_metadata: Option<ProviderMetadata>,
}

/// 流式事件聚合器
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    /// 警告列表
    pub warnings: Vec<ModelWarning>,
    /// 响应元数据
    pub response: Option<ResponseMetadata>,
    /// 文本块累加器（按 id 索引文本缓冲）
    text_buffers: HashMap<String, (String, Option<ProviderMetadata>)>,
    /// 思考链累加器（按 id 索引）
    reasoning_buffers: HashMap<String, (String, Option<ProviderMetadata>)>,
    /// 工具入参累加器（按 id 索引）
    tool_input_buffers: HashMap<String, ToolInputAccumulator>,
    /// 已经完成的有序输出内容
    pub content: Vec<OutputContent>,
    /// 结束原因
    pub finish_reason: Option<FinishReason>,
    /// Token 消耗统计
    pub usage: Option<Usage>,
    /// 最终厂商元数据
    pub provider_metadata: Option<ProviderMetadata>,
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
                self.text_buffers
                    .insert(id, (String::new(), provider_metadata));
            }
            StreamPart::TextDelta {
                id,
                delta,
                provider_metadata,
            } => {
                let entry = self
                    .text_buffers
                    .entry(id)
                    .or_insert_with(|| (String::new(), provider_metadata.clone()));
                entry.0.push_str(&delta);
                if provider_metadata.is_some() {
                    entry.1 = provider_metadata;
                }
            }
            StreamPart::TextEnd { id, .. } => {
                if let Some((text, provider_metadata)) = self.text_buffers.remove(&id) {
                    if !text.is_empty() {
                        self.content.push(OutputContent::Text {
                            text,
                            provider_metadata,
                        });
                    }
                }
            }
            StreamPart::ReasoningStart {
                id,
                provider_metadata,
            } => {
                self.reasoning_buffers
                    .insert(id, (String::new(), provider_metadata));
            }
            StreamPart::ReasoningDelta {
                id,
                delta,
                provider_metadata,
            } => {
                let entry = self
                    .reasoning_buffers
                    .entry(id)
                    .or_insert_with(|| (String::new(), provider_metadata.clone()));
                entry.0.push_str(&delta);
                if provider_metadata.is_some() {
                    entry.1 = provider_metadata;
                }
            }
            StreamPart::ReasoningEnd { id, .. } => {
                if let Some((text, provider_metadata)) = self.reasoning_buffers.remove(&id) {
                    if !text.is_empty() {
                        self.content.push(OutputContent::Reasoning {
                            text,
                            provider_metadata,
                        });
                    }
                }
            }
            StreamPart::ToolInputStart {
                id,
                tool_name,
                provider_executed,
                dynamic,
                provider_metadata,
                ..
            } => {
                self.tool_input_buffers.insert(
                    id.clone(),
                    ToolInputAccumulator {
                        id,
                        tool_name,
                        input_buffer: String::new(),
                        provider_executed,
                        dynamic,
                        provider_metadata,
                    },
                );
            }
            StreamPart::ToolInputDelta { id, delta, .. } => {
                if let Some(acc) = self.tool_input_buffers.get_mut(&id) {
                    acc.input_buffer.push_str(&delta);
                }
            }
            StreamPart::ToolInputEnd { id, .. } => {
                if let Some(acc) = self.tool_input_buffers.remove(&id) {
                    self.content.push(OutputContent::ToolCall {
                        tool_call_id: acc.id,
                        tool_name: acc.tool_name,
                        input: acc.input_buffer,
                        provider_executed: acc.provider_executed,
                        dynamic: acc.dynamic,
                        provider_metadata: acc.provider_metadata,
                    });
                }
            }
            StreamPart::ToolCall {
                tool_call_id,
                tool_name,
                input,
                provider_executed,
                dynamic,
                provider_metadata,
            } => {
                self.content.push(OutputContent::ToolCall {
                    tool_call_id,
                    tool_name,
                    input,
                    provider_executed,
                    dynamic,
                    provider_metadata,
                });
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
                self.finish_reason = Some(finish_reason);
                if provider_metadata.is_some() {
                    self.provider_metadata = provider_metadata;
                }
            }
            StreamPart::Error { .. } | StreamPart::Raw { .. } => {}
        }
    }

    /// 结束聚合，生成最终的 GenerateResult
    pub fn finish(mut self) -> GenerateResult {
        // 刷新所有未显式通过 end 闭合的残余 buffer
        for (_id, (text, metadata)) in self.text_buffers {
            if !text.is_empty() {
                self.content.push(OutputContent::Text {
                    text,
                    provider_metadata: metadata,
                });
            }
        }
        for (_id, (text, metadata)) in self.reasoning_buffers {
            if !text.is_empty() {
                self.content.push(OutputContent::Reasoning {
                    text,
                    provider_metadata: metadata,
                });
            }
        }
        for (_id, acc) in self.tool_input_buffers {
            self.content.push(OutputContent::ToolCall {
                tool_call_id: acc.id,
                tool_name: acc.tool_name,
                input: acc.input_buffer,
                provider_executed: acc.provider_executed,
                dynamic: acc.dynamic,
                provider_metadata: acc.provider_metadata,
            });
        }

        let finish_reason = self.finish_reason.unwrap_or(FinishReason {
            unified: UnifiedFinishReason::Stop,
            raw: None,
        });

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
}
