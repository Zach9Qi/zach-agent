//! 提示词消息内容块细分定义

use crate::file::FileData;
use crate::options::ProviderOptions;
use crate::tool::ToolResultOutput;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 用户消息内容块
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserPart {
    /// 纯文本块
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 多模态附件（图片、文档、音频、视频等）
    File {
        media_type: String,
        data: FileData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

impl UserPart {
    /// 构造纯文本块
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            provider_options: None,
        }
    }

    /// 构造文件附件块
    pub fn file(media_type: impl Into<String>, data: FileData) -> Self {
        Self::File {
            media_type: media_type.into(),
            data,
            filename: None,
            provider_options: None,
        }
    }
}

/// 助手消息内容块（支持历史推理、工具调用与结果回放）
///
/// 在多轮对话中，助手历史消息不仅包含普通文本，还可能包含思考链、
/// 工具调用指令、服务端内建工具的执行结果以及生成的多模态文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantPart {
    /// 模型生成的普通文本内容
    Text {
        /// 文本正文内容
        text: String,
        /// 厂商专有配置或元数据（如缓存标记、厂商自定义路由参数等）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 模型历史思考链 / 深度推理过程回放
    ///
    /// 适用于具备推理能力的大模型（如 DeepSeek-R1、OpenAI o1/o3、Claude Extended Thinking 等）。
    /// 在支持上下文回传思考链的厂商中，回放此块以保持后续轮次的推理连贯性。
    Reasoning {
        /// 推理过程与思考内容正文
        text: String,
        /// 厂商专有思考配置（如思考签名 signature、redacted_thinking 标记等）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 模型在推理过程中伴随生成的多模态文件回放
    ReasoningFile {
        /// 媒体资源 MIME 类型（如 "image/png"、"application/pdf"）
        media_type: String,
        /// 文件的实际二进制或 Base64 编码数据载荷
        data: FileData,
        /// 厂商专有扩展选项
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 模型发出的工具调用指令
    ToolCall {
        /// 本次工具调用的全局唯一标识符（由模型生成，用于后续与工具执行结果一对一关联绑定）
        tool_call_id: String,
        /// 目标工具名称（需与注册的工具名称严格一致，如 "bash"、"file_search"）
        tool_name: String,
        /// 工具输入参数，已校验并结构化为 JSON Object 的入参数据
        input: Value,
        /// 标识该工具是否由模型厂商服务端直接托管执行（如 Gemini 搜索、Claude 代码解释器），而非客户端本地执行
        #[serde(default)]
        provider_executed: bool,
        /// 厂商专有扩展选项（如特定厂商的工具调用附加上下文参数）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 工具执行结果回放
    ///
    /// 主要用于厂商服务端自执行工具（`provider_executed = true`）的结果回放，
    /// 或某些协议（如 Anthropic 原生 API）支持直接内联在助手消息中的工具执行产出。
    ToolResult {
        /// 关联的工具调用 ID（与触发该结果的 `tool_call_id` 严格一致）
        tool_call_id: String,
        /// 执行该操作的工具名称
        tool_name: String,
        /// 工具执行的输出结果载荷（包含纯文本、结构化 JSON、错误或拒绝执行状态）
        output: ToolResultOutput,
        /// 厂商专有扩展选项
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 模型生成或产出的多模态文件回放
    File {
        /// 媒体资源 MIME 类型（如 "image/jpeg"、"audio/wav"）
        media_type: String,
        /// 文件数据载荷
        data: FileData,
        /// 可选的文件名或展示标签
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// 厂商专有扩展选项
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 厂商专有的自定义扩展内容块
    ///
    /// 用于容纳非标准协议或特定模型专有的特殊块（如引用卡片、特定交互组件等）。
    Custom {
        /// 自定义内容块的类别标识（如 "citation"、"annotation"）
        kind: String,
        /// 自定义块的具体结构化数据
        data: Value,
        /// 厂商专有扩展选项
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

impl AssistantPart {
    /// 构造文本块
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            provider_options: None,
        }
    }

    /// 构造思考块
    pub fn reasoning(text: impl Into<String>) -> Self {
        Self::Reasoning {
            text: text.into(),
            provider_options: None,
        }
    }

    /// 构造工具调用块
    pub fn tool_call(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: Value,
    ) -> Self {
        Self::ToolCall {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            input,
            provider_executed: false,
            provider_options: None,
        }
    }
}

/// 工具消息内容块
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolPart {
    /// 工具执行结果
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        output: ToolResultOutput,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 用户对厂商侧执行工具的审批答复
    ToolApprovalResponse {
        approval_id: String,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

impl ToolPart {
    /// 构造工具执行成功文本结果
    pub fn result_text(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output: ToolResultOutput::text(text),
            provider_options: None,
        }
    }

    /// 构造工具执行 JSON 结果
    pub fn result_json(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        json: Value,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output: ToolResultOutput::json(json),
            provider_options: None,
        }
    }
}
