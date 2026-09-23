//! 流式事件切片（StreamPart）定义

use crate::file::FileData;
use crate::options::{ModelWarning, ProviderMetadata};
use crate::response::{FinishReason, ResponseMetadata, SourceContent, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 流式传输事件分块
///
/// 统一抽象各 LLM 厂商的 Server-Sent Events (SSE) 流式传输事件，
/// 涵盖流生命周期、文本/思考链增量、工具参数流、服务端工具执行结果及审批流。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StreamPart {
    // -----------------------------------------------------------------------
    // 流生命周期与元数据
    // -----------------------------------------------------------------------
    /// 流开始事件，包含前置校验或参数转换产生的警告信息
    StreamStart {
        /// 模型调用前置警告列表（如使用了不支持的参数或触发了降级策略）
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<ModelWarning>,
    },
    /// 响应元数据就绪事件（包含响应 ID、实际模型标识、创建时间戳等）
    ResponseMetadata(ResponseMetadata),

    // -----------------------------------------------------------------------
    // 文本正文流
    // -----------------------------------------------------------------------
    /// 文本块生成开始事件
    TextStart {
        /// 该文本块的唯一标识符（用于在交替输出或并行分块时区分不同文本段）
        id: String,
        /// 厂商专有元数据（如缓存命中状态等）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 文本增量片段事件（流式高频触发）
    TextDelta {
        /// 关联的文本块标识符，与 [`StreamPart::TextStart`] 的 `id` 对应
        id: String,
        /// 本次增量到达的文本字符片段
        delta: String,
        /// 本分块附带的厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 文本块生成结束事件
    TextEnd {
        /// 结束的文本块标识符
        id: String,
        /// 文本块结束时附带的厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 思考/推理过程流
    // -----------------------------------------------------------------------
    /// 思考链生成开始事件（针对 DeepSeek-R1、OpenAI o1/o3、Claude Extended Thinking 等推理模型）
    ReasoningStart {
        /// 该思考链块的唯一标识符
        id: String,
        /// 厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 思考链增量片段事件（流式高频触发）
    ReasoningDelta {
        /// 关联的思考链块标识符，与 [`StreamPart::ReasoningStart`] 的 `id` 对应
        id: String,
        /// 本次增量到达的思考链文本片段
        delta: String,
        /// 本分块附带的厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 思考链生成结束事件
    ReasoningEnd {
        /// 结束的思考链块标识符
        id: String,
        /// 思考链结束时附带的厂商元数据（如思考签名 signature、redacted_thinking 标记等）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 工具调用入参流式传输
    // -----------------------------------------------------------------------
    /// 工具入参流式传输开始事件
    ToolInputStart {
        /// 本次工具调用的唯一标识符（即 `tool_call_id`）
        id: String,
        /// 目标工具名称（需与注册工具名称一致）
        tool_name: String,
        /// 标识该工具是否由模型厂商服务端托管直接执行（如 Gemini 搜索、Claude 代码解释器）
        #[serde(default)]
        provider_executed: bool,
        /// 标识是否为动态加载或按需发现的工具
        #[serde(default)]
        dynamic: bool,
        /// 工具的可选展示标题（常用于前端 UI 展示）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// 厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具入参 JSON 字符串增量片段（流式高频触发）
    ToolInputDelta {
        /// 关联的工具调用标识符，与 [`StreamPart::ToolInputStart`] 的 `id` 对应
        id: String,
        /// 本次增量到达的工具入参 JSON 字符片段（需原地拼接入参缓冲区）
        delta: String,
        /// 本分块附带的厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具入参传输结束事件
    ToolInputEnd {
        /// 结束的工具调用标识符
        id: String,
        /// 工具调用结束时附带的厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 完整块与结果
    // -----------------------------------------------------------------------
    /// 一次性完整工具调用事件
    ///
    /// 部分厂商或代理不分片传输工具参数，或者在流末尾补充发送完整结构。
    ToolCall {
        /// 工具调用全局唯一标识符
        tool_call_id: String,
        /// 目标工具名称
        tool_name: String,
        /// 完整的工具入参 JSON 格式字符串
        input: String,
        /// 标识该工具是否由模型厂商服务端直接托管执行
        #[serde(default)]
        provider_executed: bool,
        /// 标识是否为动态工具
        #[serde(default)]
        dynamic: bool,
        /// 厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 厂商侧托管执行的工具结果事件
    ///
    /// 当 `provider_executed = true` 时，服务端内置工具（如网页搜索、代码执行）产生的结果通过此事件回传。
    ToolResult {
        /// 关联的工具调用全局唯一标识符
        tool_call_id: String,
        /// 工具名称
        tool_name: String,
        /// 工具执行的结构化输出结果
        result: Value,
        /// 标识工具执行是否发生异常
        #[serde(default)]
        is_error: bool,
        /// 标识是否为流式执行过程中的临时/中间态结果（如长任务进度汇报，非最终结果）
        #[serde(default)]
        preliminary: bool,
        /// 标识是否为动态工具
        #[serde(default)]
        dynamic: bool,
        /// 厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具执行审批请求事件
    ///
    /// 适用于服务端敏感工具调用需人机交互审批介入（Human-in-the-loop）的场景。
    ToolApprovalRequest {
        /// 本次审批请求的唯一流水号
        approval_id: String,
        /// 需审批的目标工具调用标识符
        tool_call_id: String,
        /// 厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 流式输出中生成的多模态文件事件
    File {
        /// 媒体资源 MIME 类型（如 "image/png"、"application/pdf"）
        media_type: String,
        /// 文件的实际二进制或 Base64 编码数据载荷
        data: FileData,
        /// 厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 推理过程中生成的多模态文件事件
    ReasoningFile {
        /// 媒体资源 MIME 类型
        media_type: String,
        /// 文件数据载荷
        data: FileData,
        /// 厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 检索增强（RAG）或联网搜索引用的数据源事件
    Source(SourceContent),
    /// 厂商特有自定义内容事件（透传厂商专有扩展块）
    Custom {
        /// 自定义块的类型标识
        kind: String,
        /// 具体的结构化数据载荷
        data: Value,
        /// 厂商专有元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 流结束与错误
    // -----------------------------------------------------------------------
    /// 生成流正常完成事件
    Finish {
        /// 本次请求消耗的 Token 统计用量（包含 prompt、completion 及 reasoning tokens）
        usage: Usage,
        /// 归一化的模型停止原因（如 stop, tool_calls, length, content_filter 等）
        finish_reason: FinishReason,
        /// 最终厂商响应级元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 厂商下发的错误事件或单个分块解析失败（不终止流，可多次出现）
    Error {
        /// 格式化后的错误可读描述信息
        message: String,
        /// 厂商返回的原始错误信息 JSON 载荷（供深度排错使用）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw: Option<Value>,
    },
    /// 开启 `include_raw_chunks` 时透出的厂商底层原始 SSE 数据块
    Raw {
        /// 厂商原始未加工的数据载荷
        raw_value: Value,
    },
}
