//! 错误的可序列化投影

use super::kind::ErrorKind;
use super::model::{write_summary, ModelError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt;
use std::time::Duration;

/// 错误的可序列化投影：跨越事件、流、持久化边界时只传它
///
/// 由 `From<ModelError>` 生成，这是把错误拍平为数据的唯一入口：`causes` 保留完整的
/// `source` 链，`Display` 输出与 [`super::report`] 一致的完整描述。
/// 反向的 `From<ErrorInfo> for ModelError` 保留类别与上下文，原因链以文本形式还原。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// 错误类别
    pub kind: ErrorKind,
    /// 本层描述，不含原因链
    pub message: String,
    /// `source` 链，由外到内
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub causes: Vec<String>,
    /// 细分错误码（厂商错误码、特性名等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 厂商标识
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// HTTP 状态码
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// 服务端建议的重试等待毫秒数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// 原始错误载荷（厂商错误正文、出错的事件等），供深度排错
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

impl ErrorInfo {
    /// 以类别和描述构造，其余字段为空
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            causes: Vec::new(),
            code: None,
            provider: None,
            status: None,
            retry_after_ms: None,
            raw: None,
        }
    }

    /// 原样重试是否可能成功，见 [`ErrorKind::is_retryable`]
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }
}

impl fmt::Display for ErrorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_summary(
            f,
            self.kind,
            self.provider.as_deref(),
            &self.message,
            self.status,
        )?;
        for cause in &self.causes {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl From<&ModelError> for ErrorInfo {
    fn from(error: &ModelError) -> Self {
        let causes = std::iter::successors(error.source(), |&cause| cause.source())
            .map(ToString::to_string)
            .collect();
        Self {
            kind: error.kind(),
            message: error.message().to_owned(),
            causes,
            code: error.code().map(str::to_owned),
            provider: error.provider().map(str::to_owned),
            status: error.status(),
            retry_after_ms: error
                .retry_after()
                .map(|after| u64::try_from(after.as_millis()).unwrap_or(u64::MAX)),
            raw: error.raw().cloned(),
        }
    }
}

impl From<ModelError> for ErrorInfo {
    fn from(error: ModelError) -> Self {
        Self::from(&error)
    }
}

impl From<ErrorInfo> for ModelError {
    fn from(info: ErrorInfo) -> Self {
        let mut error = ModelError::new(info.kind, info.message);
        if let Some(code) = info.code {
            error = error.with_code(code);
        }
        if let Some(provider) = info.provider {
            error = error.with_provider(provider);
        }
        if let Some(status) = info.status {
            error = error.with_status(status);
        }
        if let Some(millis) = info.retry_after_ms {
            error = error.with_retry_after(Duration::from_millis(millis));
        }
        if let Some(raw) = info.raw {
            error = error.with_raw(raw);
        }
        match Cause::chain(info.causes) {
            Some(source) => error.with_source(source),
            None => error,
        }
    }
}

/// 由 [`ErrorInfo::causes`] 还原的原因链节点，只保留文本
#[derive(Debug)]
struct Cause {
    message: String,
    source: Option<Box<Cause>>,
}

impl Cause {
    /// 由外到内的文本还原为链，外层在前
    fn chain(causes: Vec<String>) -> Option<Box<Cause>> {
        causes.into_iter().rev().fold(None, |source, message| {
            Some(Box::new(Cause { message, source }))
        })
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for Cause {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}
