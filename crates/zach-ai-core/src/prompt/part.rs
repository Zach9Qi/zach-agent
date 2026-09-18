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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantPart {
    /// 模型生成的普通文本
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 模型历史思考链/推理过程回放
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 推理过程中生成的文件回放
    ReasoningFile {
        media_type: String,
        data: FileData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 模型发出的工具调用
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        /// 工具输入参数，已校验或格式化的 JSON Value
        input: Value,
        /// 是否由厂商直接执行
        #[serde(default)]
        provider_executed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 工具执行结果回放（部分厂商支持放在 assistant 消息内）
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        output: ToolResultOutput,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 产生的文件回放
    File {
        media_type: String,
        data: FileData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 厂商特有自定义块
    Custom {
        kind: String,
        data: Value,
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
