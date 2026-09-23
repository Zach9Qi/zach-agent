//! 流式分块累加器（将 StreamPart 流还原为 GenerateResult）
//!
//! 文本、推理和工具入参按**第一次出现**的位置占位，后续增量原地拼接。
//! 因此未发送 End、或 End 晚于其他内容到达时，最终顺序仍与流的起始顺序一致。

mod slots;

use std::collections::{HashMap, HashSet};

use crate::options::{ModelWarning, ProviderMetadata};
use crate::response::{
    FinishReason, GenerateResult, OutputContent, ResponseMetadata, UnifiedFinishReason, Usage,
};
use crate::stream::part::StreamPart;
use serde_json::Value;

/// 流式事件聚合器
///
/// 内部维护按 id 定位内容块的索引，因此不暴露可写字段；
/// 处理过程中可通过只读访问器观察当前状态，调用 [`Self::finish`] 得到最终结果。
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    warnings: Vec<ModelWarning>,
    response: Option<ResponseMetadata>,
    /// 按首次出现顺序排列的实时内容。
    /// 只收到 Start、尚无增量且无元数据的文本或推理段会暂时是空字符串，`finish` 时剔除。
    content: Vec<OutputContent>,
    usage: Option<Usage>,
    provider_metadata: Option<ProviderMetadata>,

    text_index: HashMap<String, usize>,
    reasoning_index: HashMap<String, usize>,
    tool_index: HashMap<String, usize>,
    /// 按 `tool_call_id` 定位工具结果块，后续（中间态或最终）结果原地替换
    tool_result_index: HashMap<String, usize>,
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

    /// 已收集的警告列表
    pub fn warnings(&self) -> &[ModelWarning] {
        &self.warnings
    }

    /// 已收到的响应元数据
    pub fn response(&self) -> Option<&ResponseMetadata> {
        self.response.as_ref()
    }

    /// 当前按首次出现顺序排列的内容块（含尚未收到增量的空块）
    pub fn content(&self) -> &[OutputContent] {
        &self.content
    }

    /// 流是否已收尾（收到 `Finish` 事件）
    ///
    /// `Error` 事件不终止流，之后仍可能到达内容或 `Finish`，因此不计入收尾。
    /// 为 `false` 时调用 [`Self::finish`]：出过错会得到 [`UnifiedFinishReason::Error`]，
    /// 否则得到 [`UnifiedFinishReason::Unknown`]，提示调用方该结果可能是被静默截断的半截内容。
    pub fn is_complete(&self) -> bool {
        self.explicit_finish.is_some()
    }

    /// 当前已解析的结束原因。仅在流已收尾（见 [`Self::is_complete`]）后有值。
    pub fn finish_reason(&self) -> Option<FinishReason> {
        self.is_complete().then(|| self.resolved_finish_reason())
    }

    /// 已收到的 Token 用量
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// 已收到的最终厂商元数据
    pub fn provider_metadata(&self) -> Option<&ProviderMetadata> {
        self.provider_metadata.as_ref()
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
                self.apply_tool_result(OutputContent::ToolResult {
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
                if provider_metadata.is_some() {
                    self.provider_metadata = provider_metadata;
                }
            }
            StreamPart::Error { message, raw } => {
                self.stream_error = Some(describe_stream_error(message, raw));
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
            // 既无 Finish 也无 Error：流被静默截断，不能伪装成正常停止
            (None, None) => FinishReason {
                unified: UnifiedFinishReason::Unknown,
                raw: None,
            },
        }
    }
}

/// 剔除既无正文也无厂商元数据的空块。
///
/// 只带元数据的空块必须保留：例如 Anthropic 的 `redacted_thinking` 正文为空，
/// 全部信息都在元数据里，丢弃会导致下一轮回放缺块。
fn keep_in_final_content(part: &OutputContent) -> bool {
    match part {
        OutputContent::Text {
            text,
            provider_metadata,
        }
        | OutputContent::Reasoning {
            text,
            provider_metadata,
        } => !text.is_empty() || provider_metadata.is_some(),
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
