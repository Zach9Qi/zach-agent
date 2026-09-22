//! 模型生成的有序内容块定义

use crate::file::FileData;
use crate::options::ProviderMetadata;
use crate::prompt::part::AssistantPart;
use crate::tool::ToolResultOutput;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 引用来源类型
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source_type", rename_all = "snake_case")]
pub enum SourceContent {
    /// 网页链接引用
    Url {
        id: String,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 文档引用
    Document {
        id: String,
        media_type: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
}

/// 模型生成的内容块（输出侧）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputContent {
    /// 文本正文
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 思考/推理链
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 模型发出的工具调用请求（入参为原始 JSON 字符串）
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        input: String,
        #[serde(default)]
        provider_executed: bool,
        #[serde(default)]
        dynamic: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 厂商侧已直接执行的工具结果（如服务端 MCP）
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        result: Value,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        preliminary: bool,
        #[serde(default)]
        dynamic: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 厂商对服务端执行工具发起的审批请求
    ToolApprovalRequest {
        approval_id: String,
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 模型生成的文件产物
    File {
        media_type: String,
        data: FileData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 推理过程中生成的文件
    ReasoningFile {
        media_type: String,
        data: FileData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 生成引用的数据源
    Source(SourceContent),
    /// 厂商特有自定义块
    Custom {
        kind: String,
        data: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
}

impl OutputContent {
    /// 提取纯文本内容（若是 Text 变体）
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    /// 提取思考链文本（若是 Reasoning 变体）
    pub fn as_reasoning(&self) -> Option<&str> {
        match self {
            Self::Reasoning { text, .. } => Some(text),
            _ => None,
        }
    }

    /// 将输出块转换为下一轮 Prompt 可回放的 AssistantPart
    pub fn into_assistant_part(self) -> Option<AssistantPart> {
        match self {
            Self::Text {
                text,
                provider_metadata,
            } => Some(AssistantPart::Text {
                text,
                provider_options: provider_metadata,
            }),
            Self::Reasoning {
                text,
                provider_metadata,
            } => Some(AssistantPart::Reasoning {
                text,
                provider_options: provider_metadata,
            }),
            Self::ReasoningFile {
                media_type,
                data,
                provider_metadata,
            } => Some(AssistantPart::ReasoningFile {
                media_type,
                data,
                provider_options: provider_metadata,
            }),
            Self::File {
                media_type,
                data,
                provider_metadata,
            } => Some(AssistantPart::File {
                media_type,
                data,
                filename: None,
                provider_options: provider_metadata,
            }),
            Self::ToolCall {
                tool_call_id,
                tool_name,
                input,
                provider_executed,
                provider_metadata,
                ..
            } => Some(AssistantPart::ToolCall {
                tool_call_id,
                tool_name,
                input: parse_tool_input(&input),
                provider_executed,
                provider_options: provider_metadata,
            }),
            Self::ToolResult {
                tool_call_id,
                tool_name,
                result,
                is_error,
                preliminary,
                provider_metadata,
                ..
            } => {
                if preliminary {
                    None
                } else {
                    Some(AssistantPart::ToolResult {
                        tool_call_id,
                        tool_name,
                        output: tool_result_output(is_error, result),
                        provider_options: provider_metadata,
                    })
                }
            }
            Self::Custom {
                kind,
                data,
                provider_metadata,
            } => Some(AssistantPart::Custom {
                kind,
                data,
                provider_options: provider_metadata,
            }),
            // 审批请求、引用源不进入下一轮历史
            _ => None,
        }
    }
}

/// 将流式拼接得到的原始入参字符串解析为结构化 JSON。
///
/// - 空白串（无参工具在流式场景下的常态）视为空对象 `{}`；
/// - 合法 JSON 原样解析；
/// - 非法 JSON（截断、畸形）保留为字符串，交由上层决定如何处理。
pub fn parse_tool_input(input: &str) -> Value {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| Value::String(input.to_string()))
}

fn tool_result_output(is_error: bool, result: Value) -> ToolResultOutput {
    match (is_error, result) {
        (false, Value::String(text)) => ToolResultOutput::text(text),
        (false, value) => ToolResultOutput::json(value),
        (true, Value::String(text)) => ToolResultOutput::error_text(text),
        (true, value) => ToolResultOutput::ErrorJson {
            value,
            provider_options: None,
        },
    }
}
