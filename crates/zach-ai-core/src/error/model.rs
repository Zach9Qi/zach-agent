//! 模型调用错误

use super::kind::ErrorKind;
use serde_json::Value;
use std::error::Error;
use std::fmt;
use std::time::Duration;

/// 底层错误的统一装箱形式
type BoxError = Box<dyn Error + Send + Sync>;

/// 模型调用错误
///
/// 由 [`ErrorKind`]、统一的上下文字段与 `source` 链组成。所有类别共用同一组上下文字段，
/// 新增类别不需要新增字段或构造函数。整体装箱，`Result<T, ModelError>` 只占一个指针；
/// 字段私有，新增上下文不破坏 API。
///
/// 用 [`Self::new`] 或类别快捷构造函数创建，再用 `with_*` 按需附加上下文：
///
/// ```
/// use zach_ai_core::{ErrorKind, ModelError};
///
/// let error = ModelError::new(ErrorKind::Unavailable, "Overloaded")
///     .with_provider("anthropic")
///     .with_status(529)
///     .with_code("overloaded_error");
/// assert!(error.is_retryable());
/// assert_eq!(error.to_string(), "[anthropic] 服务暂不可用: Overloaded (HTTP 529)");
/// ```
pub struct ModelError(Box<Inner>);

#[derive(Debug)]
struct Inner {
    kind: ErrorKind,
    /// 本层事实，不含 `source` 的内容
    message: String,
    /// 细分标识：厂商错误码（`overloaded_error`）或特性名（`reasoning_effort.medium`）
    code: Option<String>,
    provider: Option<String>,
    status: Option<u16>,
    retry_after: Option<Duration>,
    raw: Option<Value>,
    source: Option<BoxError>,
}

impl ModelError {
    /// 以类别和本层描述构造
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self(Box::new(Inner {
            kind,
            message: message.into(),
            code: None,
            provider: None,
            status: None,
            retry_after: None,
            raw: None,
            source: None,
        }))
    }

    /// 本地校验失败，请求没有发出
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidRequest, message)
    }

    /// 模型或适配器不支持某项特性；特性名同时作为错误码，便于程序判断
    pub fn unsupported(feature: impl Into<String>, details: Option<String>) -> Self {
        let feature = feature.into();
        let message = match details {
            Some(details) => format!("{feature}（{details}）"),
            None => feature.clone(),
        };
        Self::new(ErrorKind::Unsupported, message).with_code(feature)
    }

    /// 响应无法解析或违反协议
    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidResponse, message)
    }

    /// 连接失败、读取中断或超时
    pub fn transport(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Transport, message)
    }

    /// 附加细分错误码
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.0.code = Some(code.into());
        self
    }

    /// 附加厂商标识
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.0.provider = Some(provider.into());
        self
    }

    /// 附加 HTTP 状态码
    pub fn with_status(mut self, status: u16) -> Self {
        self.0.status = Some(status);
        self
    }

    /// 附加服务端建议的重试等待时长
    pub fn with_retry_after(mut self, after: Duration) -> Self {
        self.0.retry_after = Some(after);
        self
    }

    /// 附加原始错误载荷（厂商错误正文、出错的事件等），供深度排错
    pub fn with_raw(mut self, raw: Value) -> Self {
        self.0.raw = Some(raw);
        self
    }

    /// 附加底层错误作为 `source`
    pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
        self.0.source = Some(source.into());
        self
    }

    /// 错误类别
    pub fn kind(&self) -> ErrorKind {
        self.0.kind
    }

    /// 原样重试是否可能成功，见 [`ErrorKind::is_retryable`]
    pub fn is_retryable(&self) -> bool {
        self.0.kind.is_retryable()
    }

    /// 本层描述，不含 `source` 的内容
    pub fn message(&self) -> &str {
        &self.0.message
    }

    /// 细分错误码
    pub fn code(&self) -> Option<&str> {
        self.0.code.as_deref()
    }

    /// 厂商标识
    pub fn provider(&self) -> Option<&str> {
        self.0.provider.as_deref()
    }

    /// HTTP 状态码
    pub fn status(&self) -> Option<u16> {
        self.0.status
    }

    /// 服务端建议的重试等待时长
    pub fn retry_after(&self) -> Option<Duration> {
        self.0.retry_after
    }

    /// 原始错误载荷
    pub fn raw(&self) -> Option<&Value> {
        self.0.raw.as_ref()
    }
}

/// 统一的摘要格式：`[厂商] 类别: 描述 (HTTP 状态码)`，缺失的部分省略
pub(super) fn write_summary(
    f: &mut fmt::Formatter<'_>,
    kind: ErrorKind,
    provider: Option<&str>,
    message: &str,
    status: Option<u16>,
) -> fmt::Result {
    if let Some(provider) = provider {
        write!(f, "[{provider}] ")?;
    }
    write!(f, "{kind}")?;
    if !message.is_empty() {
        write!(f, ": {message}")?;
    }
    if let Some(status) = status {
        write!(f, " (HTTP {status})")?;
    }
    Ok(())
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = &self.0;
        write_summary(
            f,
            inner.kind,
            inner.provider.as_deref(),
            &inner.message,
            inner.status,
        )
    }
}

impl fmt::Debug for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = &self.0;
        f.debug_struct("ModelError")
            .field("kind", &inner.kind)
            .field("message", &inner.message)
            .field("code", &inner.code)
            .field("provider", &inner.provider)
            .field("status", &inner.status)
            .field("retry_after", &inner.retry_after)
            .field("raw", &inner.raw)
            .field("source", &inner.source)
            .finish()
    }
}

impl Error for ModelError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0
            .source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}
