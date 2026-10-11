//! 厂商错误分类：HTTP 错误响应与流内 error 事件共用同一套规则
//!
//! 先看厂商错误码（由 [`ErrorDialect`] 识别），认不出再按 HTTP 状态码兜底。
//! 两条路径共用同一个方言，同一个故障（例如过载）不会因为以 HTTP 529 还是流内事件到达
//! 而得到不同类别，也就不会出现一种重试、一种不重试。

use reqwest::header::HeaderMap;
use serde_json::Value;
use std::time::Duration;
use zach_ai_core::{ErrorKind, ModelError};

/// 厂商错误方言：如何从错误对象读出错误码，以及错误码对应的类别
pub(crate) struct ErrorDialect {
    /// 从错误对象中读出错误码
    pub(crate) code: fn(&Value) -> Option<&str>,
    /// 错误码对应的类别；返回 `None` 表示不认识，交给状态码兜底
    pub(crate) kind: fn(code: &str, error: &Value) -> Option<ErrorKind>,
}

impl ErrorDialect {
    /// 由厂商错误对象构造错误：流内 error 事件、2xx 响应里的错误对象
    ///
    /// 这类错误没有可参考的状态码，错误码认不出时归为 [`ErrorKind::Rejected`]：
    /// 不了解的错误不重试，避免把确定性失败重复发送。原始载荷由调用方附加。
    pub(crate) fn error(&self, provider: &str, error: &Value) -> ModelError {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("厂商未给出错误描述");
        self.classify(provider, error, message.to_owned(), ErrorKind::Rejected)
    }

    fn classify(
        &self,
        provider: &str,
        error: &Value,
        message: String,
        fallback: ErrorKind,
    ) -> ModelError {
        let code = (self.code)(error);
        let kind = code
            .and_then(|code| (self.kind)(code, error))
            .unwrap_or(fallback);
        let classified = ModelError::new(kind, message).with_provider(provider);
        match code {
            Some(code) => classified.with_code(code),
            None => classified,
        }
    }
}

/// 非 2xx 响应的错误：厂商错误码优先，其次按状态码分类，并带上 `Retry-After`
pub(crate) fn http_error(
    dialect: &ErrorDialect,
    provider: &str,
    status: u16,
    headers: &HeaderMap,
    body: &[u8],
) -> ModelError {
    let error =
        body_error(dialect, provider, body, kind_for_status(status), None).with_status(status);
    match retry_after(headers) {
        Some(after) => error.with_retry_after(after),
        None => error,
    }
}

/// 期望事件流却收到文档（网关错误页，或把流式请求当普通请求处理）
///
/// 状态码是 2xx，属于协议违约；正文里若有能识别的厂商错误，以厂商的类别为准。
pub(crate) fn not_event_stream(
    dialect: &ErrorDialect,
    provider: &str,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> ModelError {
    let context = format!("期望 text/event-stream，收到 {content_type}");
    body_error(
        dialect,
        provider,
        body,
        ErrorKind::InvalidResponse,
        Some(&context),
    )
    .with_status(status)
}

/// 状态码兜底：401/403 鉴权，429 限流，408 与 5xx（含 Anthropic 的 529 过载）暂不可用，
/// 其余是服务端对请求本身的拒绝
fn kind_for_status(status: u16) -> ErrorKind {
    match status {
        401 | 403 => ErrorKind::Authentication,
        429 => ErrorKind::RateLimited,
        408 | 500..=599 => ErrorKind::Unavailable,
        _ => ErrorKind::Rejected,
    }
}

/// 解析错误正文：JSON 的错误对象在 `error` 字段或顶层，交给方言识别；非 JSON 保留原文
fn body_error(
    dialect: &ErrorDialect,
    provider: &str,
    body: &[u8],
    fallback: ErrorKind,
    context: Option<&str>,
) -> ModelError {
    let raw = serde_json::from_slice::<Value>(body).ok();
    let object = raw.as_ref().map(|raw| {
        raw.get("error")
            .filter(|error| error.is_object())
            .unwrap_or(raw)
    });
    let message = object
        .and_then(|object| object.get("message"))
        // 部分网关返回 `{"error": "..."}`
        .or_else(|| raw.as_ref()?.get("error"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| body_text(body));
    let message = match context {
        Some(context) => format!("{context}: {message}"),
        None => message,
    };
    let error = match object {
        Some(object) => dialect.classify(provider, object, message, fallback),
        None => ModelError::new(fallback, message).with_provider(provider),
    };
    match raw {
        Some(raw) => error.with_raw(raw),
        None => error,
    }
}

fn body_text(body: &[u8]) -> String {
    if body.is_empty() {
        "响应正文为空".to_owned()
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

/// 服务端建议的重试等待：优先 `retry-after-ms`（OpenAI），其次 `retry-after` 秒数
///
/// HTTP 日期格式的 `retry-after` 不解析，退回调用方的退避策略。
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let number =
        |name: &str| -> Option<f64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };
    let seconds = number("retry-after-ms")
        .map(|millis| millis / 1000.0)
        .or_else(|| number("retry-after"))?;
    Duration::try_from_secs_f64(seconds).ok()
}

#[cfg(test)]
mod tests;
