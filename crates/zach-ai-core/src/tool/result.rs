//! 工具执行结果与载荷类型

use crate::file::FileData;
use crate::options::ProviderOptions;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 单次工具调用的执行结果载荷
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultOutput {
    /// 直接发给模型的纯文本结果
    Text {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 结构化 JSON 结果
    Json {
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 用户或系统拒绝执行该工具
    ExecutionDenied {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 纯文本形式的错误信息
    ErrorText {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 结构化 JSON 形式的错误信息
    ErrorJson {
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 复合结果（支持文本、文件与自定义块组合）
    Content { value: Vec<ToolResultContentBlock> },
}

impl ToolResultOutput {
    /// 便捷构造纯文本结果
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text {
            value: value.into(),
            provider_options: None,
        }
    }

    /// 便捷构造 JSON 结果
    pub fn json(value: Value) -> Self {
        Self::Json {
            value,
            provider_options: None,
        }
    }

    /// 便捷构造拒绝执行结果
    pub fn denied(reason: Option<impl Into<String>>) -> Self {
        Self::ExecutionDenied {
            reason: reason.map(Into::into),
            provider_options: None,
        }
    }

    /// 便捷构造文本错误结果
    pub fn error_text(value: impl Into<String>) -> Self {
        Self::ErrorText {
            value: value.into(),
            provider_options: None,
        }
    }
}

/// 复合工具结果的内容块
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContentBlock {
    /// 文本块
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 产生的文件（如代码解释器生成的图表）
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
