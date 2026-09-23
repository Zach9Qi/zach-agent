//! 统一错误类型定义

use thiserror::Error;

/// 模型调用与交互过程中的统一错误类型
#[derive(Debug, Error)]
pub enum ModelError {
    /// 客户端请求参数不合法（如缺失必填项、格式校验失败等）
    #[error("请求参数无效: {0}")]
    InvalidRequest(String),

    /// 厂商或模型不支持某项特性（如某些模型不支持 tools、json schema 等）
    #[error("不支持的特性: {feature} (详情: {details:?})")]
    UnsupportedFeature {
        feature: String,
        details: Option<String>,
    },

    /// 鉴权失败（API Key 无效、过期或权限不足）
    #[error("鉴权失败: {0}")]
    Authentication(String),

    /// 触发调用限流（Rate Limit）或配额用尽
    #[error("触发限流或配额耗尽: {0}")]
    RateLimit(String),

    /// 厂商服务端返回的明确错误
    #[error("厂商 [{provider}] 返回错误: {message}")]
    ProviderError {
        provider: String,
        message: String,
        raw: Option<serde_json::Value>,
    },

    /// 流式传输中断或读取超时（不可恢复，流随即结束）
    #[error("流式传输错误: {message}")]
    StreamError {
        message: String,
        /// 底层错误（如 `reqwest::Error`），根因常在其 `source()` 链深处
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// JSON 序列化或反序列化失败
    #[error("JSON 序列化/反序列化错误: {0}")]
    Json(#[from] serde_json::Error),

    /// 工具调用解析或执行前置错误
    #[error("工具调用错误: {0}")]
    ToolExecution(String),

    /// 未归类的其他底层或网络错误
    #[error("未归类的模型调用错误: {0}")]
    Other(String),
}

impl ModelError {
    /// 该错误本身是否属于暂时性故障（限流、传输中断），重试可能成功
    ///
    /// 只判断错误性质；流中途失败时可能已产出部分内容，是否真正重试由上层决定。
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimit(_) | Self::StreamError { .. })
    }

    /// 构造厂商返回错误
    pub fn provider_error(
        provider: impl Into<String>,
        message: impl Into<String>,
        raw: Option<serde_json::Value>,
    ) -> Self {
        Self::ProviderError {
            provider: provider.into(),
            message: message.into(),
            raw,
        }
    }

    /// 构造流式传输错误，保留底层错误作为 `source`
    pub fn stream_error(
        message: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::StreamError {
            message: message.into(),
            source: Some(source.into()),
        }
    }

    /// 构造特性不支持错误
    pub fn unsupported(feature: impl Into<String>, details: Option<String>) -> Self {
        Self::UnsupportedFeature {
            feature: feature.into(),
            details,
        }
    }
}
