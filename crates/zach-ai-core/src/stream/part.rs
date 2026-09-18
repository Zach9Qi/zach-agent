//! 流式事件切片（StreamPart）定义

use crate::file::FileData;
use crate::options::{ModelWarning, ProviderMetadata};
use crate::response::{FinishReason, ResponseMetadata, SourceContent, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 流式传输事件分块
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StreamPart {
    // -----------------------------------------------------------------------
    // 流生命周期与元数据
    // -----------------------------------------------------------------------
    /// 流开始事件，包含前置校验产生的警告信息
    StreamStart {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<ModelWarning>,
    },
    /// 响应元数据就绪事件（如 response_id、model_id 等）
    ResponseMetadata(ResponseMetadata),

    // -----------------------------------------------------------------------
    // 文本正文流
    // -----------------------------------------------------------------------
    /// 文本块生成开始
    TextStart {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 文本增量片段
    TextDelta {
        id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 文本块生成结束
    TextEnd {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 思考/推理过程流
    // -----------------------------------------------------------------------
    /// 思考链生成开始
    ReasoningStart {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 思考链增量片段
    ReasoningDelta {
        id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 思考链生成结束
    ReasoningEnd {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 工具调用入参流式传输
    // -----------------------------------------------------------------------
    /// 工具入参流开始
    ToolInputStart {
        id: String,
        tool_name: String,
        #[serde(default)]
        provider_executed: bool,
        #[serde(default)]
        dynamic: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具入参 JSON 片段增量
    ToolInputDelta {
        id: String,
        delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具入参传输结束
    ToolInputEnd {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 完整块与结果
    // -----------------------------------------------------------------------
    /// 完整的工具调用（某些厂商不分片，直接一次性发出完整工具调用）
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
    /// 厂商侧执行的工具结果（如服务端 MCP）
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
    /// 工具审批请求
    ToolApprovalRequest {
        approval_id: String,
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 流中产生的文件
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
    /// 数据源引用
    Source(SourceContent),
    /// 厂商专有自定义内容
    Custom {
        kind: String,
        data: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 流结束与错误
    // -----------------------------------------------------------------------
    /// 生成流正常完成，附带 Token 消耗统计与最终原因
    Finish {
        usage: Usage,
        finish_reason: FinishReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 流中发生的错误事件
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw: Option<Value>,
    },
    /// 开启 include_raw_chunks 时透出的厂商原始数据块
    Raw {
        raw_value: Value,
    },
}
