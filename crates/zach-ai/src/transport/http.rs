//! HTTP 侧的公共逻辑：请求头拼装、流式响应校验、JSON 响应读取与非 2xx 错误分类。

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Request, Response};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;
use zach_ai_core::ModelError;

use super::Timeouts;

/// 建立连接的上限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// 适配器默认的 HTTP 客户端：只设连接超时。
///
/// 读取与整体超时按请求类型区分（见 [`Timeouts`]）：客户端级 `read_timeout` 在等待响应头
/// 阶段同样生效，会把长时间推理的非流式请求误杀。需要代理、连接池等更细的控制时
/// 用 `with_client` 传入自定义客户端。
pub(crate) fn default_client() -> Client {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .expect("默认 HTTP 客户端构建失败")
}

/// 按请求类型套用超时：非流式请求设置请求级整体超时，流式请求不设（由空闲守卫约束）。
pub(crate) fn prepare(request: &mut Request, timeouts: &Timeouts, stream: bool) {
    if !stream {
        *request.timeout_mut() = timeouts.generate;
    }
}

/// 给一个阶段加上时长守卫；超时映射为可重试的传输错误。
async fn guarded<T>(
    guard: Option<Duration>,
    label: &str,
    phase: &str,
    future: impl Future<Output = T>,
) -> Result<T, ModelError> {
    match guard {
        Some(limit) => {
            tokio::time::timeout(limit, future)
                .await
                .map_err(|_| ModelError::StreamError {
                    message: format!("{label} {phase}超过 {} 秒", limit.as_secs()),
                    source: None,
                })
        }
        None => Ok(future.await),
    }
}

/// 请求流式还是普通响应时的 `Accept` 值。
pub(crate) fn accept(stream: bool) -> &'static str {
    if stream {
        "text/event-stream"
    } else {
        "application/json"
    }
}

/// 构造标记为敏感的请求头值（凭据不进入日志与 Debug 输出）。
pub(crate) fn sensitive_header(value: &str, what: &str) -> Result<HeaderValue, ModelError> {
    let mut header = HeaderValue::from_str(value)
        .map_err(|_| ModelError::InvalidRequest(format!("{what} 包含非法 HTTP 字符")))?;
    header.set_sensitive(true);
    Ok(header)
}

/// 把 `CallOptions.headers` 并入请求头，同名覆盖；鉴权头一律标记为敏感。
pub(crate) fn extend_headers(
    headers: &mut HeaderMap,
    extra: Option<&HashMap<String, String>>,
) -> Result<(), ModelError> {
    for (name, value) in extra.into_iter().flatten() {
        let name = HeaderName::try_from(name.as_str())
            .map_err(|_| ModelError::InvalidRequest(format!("非法 HTTP 请求头名称: {name}")))?;
        let mut value = HeaderValue::from_str(value)
            .map_err(|_| ModelError::InvalidRequest(format!("非法 HTTP 请求头值: {name}")))?;
        if name == AUTHORIZATION || name == "x-api-key" {
            value.set_sensitive(true);
        }
        headers.insert(name, value);
    }
    Ok(())
}

/// 执行请求：传输失败映射为 `StreamError`，非 2xx 按状态分类。
///
/// `guard` 约束等待响应头与读取错误正文的时长（流式请求的空闲守卫）；
/// 非流式请求传 `None`，由 [`prepare`] 设置的请求级超时覆盖全程。
pub(crate) async fn execute(
    client: &Client,
    provider: &str,
    label: &str,
    request: Request,
    guard: Option<Duration>,
) -> Result<Response, ModelError> {
    let response = guarded(guard, label, "等待响应头", client.execute(request))
        .await?
        .map_err(|err| ModelError::stream_error(format!("请求 {label} 失败"), err))?;
    if response.status().is_success() {
        Ok(response)
    } else {
        let error = http_error(provider, label, response);
        Err(guarded(guard, label, "读取错误响应", error)
            .await
            .unwrap_or_else(|timeout| timeout))
    }
}

/// 流式响应应是 `text/event-stream`。
///
/// 网关返回 JSON、HTML 或纯文本说明拿到的不是流（错误页，或把流式请求当普通请求处理了），
/// 读出正文按厂商错误报告，避免错误信息丢失；缺失或 `application/octet-stream` 这类配置疏漏
/// 放行，由 SSE 解码裁决。`guard` 约束读取正文的时长（流式请求的空闲守卫）。
pub(crate) async fn require_event_stream(
    response: Response,
    provider: &str,
    label: &str,
    guard: Option<Duration>,
) -> Result<Response, ModelError> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !looks_like_a_document(&content_type) {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let body = guarded(guard, label, "读取非流式响应", response.bytes())
        .await?
        .map_err(|err| ModelError::stream_error(format!("读取 {label} 响应失败"), err))?;
    Err(match classify(provider, status, &body) {
        ModelError::ProviderError {
            provider,
            message,
            raw,
        } => ModelError::ProviderError {
            provider,
            message: format!("期望 text/event-stream，收到 {content_type}: {message}"),
            raw,
        },
        other => other,
    })
}

/// JSON、HTML 与纯文本是文档而不是事件流。
fn looks_like_a_document(content_type: &str) -> bool {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    media_type == "text/html" || media_type == "text/plain" || media_type.ends_with("json")
}

/// 读取 JSON 响应体，连同可转为字符串的响应头一起返回。
pub(crate) async fn read_json(
    response: Response,
    label: &str,
) -> Result<(Value, HashMap<String, String>), ModelError> {
    let headers = header_map(response.headers());
    let body = response
        .bytes()
        .await
        .map_err(|err| ModelError::stream_error(format!("读取 {label} 响应失败"), err))?;
    Ok((serde_json::from_slice(&body)?, headers))
}

/// 响应头转为键值表；非 UTF-8 的值跳过。
fn header_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_owned()))
        })
        .collect()
}

async fn http_error(provider: &str, label: &str, response: Response) -> ModelError {
    let status = response.status().as_u16();
    match response.bytes().await {
        Ok(body) => classify(provider, status, &body),
        Err(error) => ModelError::stream_error(format!("读取 {label} 错误响应失败"), error),
    }
}

/// 按状态码分类，错误正文优先取 `error.message` / `message`，非 JSON 正文原样保留。
///
/// 401/403 为鉴权失败；429 与 529（Anthropic 的 overloaded_error）为限流；
/// 408 与其余 5xx 是服务端暂时性故障，可重试；其他状态是服务端对请求本身的明确拒绝。
fn classify(provider: &str, status: u16, body: &[u8]) -> ModelError {
    let raw = serde_json::from_slice::<Value>(body).ok();
    let message = raw
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
        })
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if body.is_empty() {
                format!("HTTP {status}")
            } else {
                String::from_utf8_lossy(body).into_owned()
            }
        });
    match status {
        401 | 403 => ModelError::Authentication(message),
        429 | 529 => ModelError::RateLimit(message),
        408 | 500..=599 => ModelError::server_error(provider, status, message, raw),
        _ => ModelError::provider_error(provider, message, raw),
    }
}

#[cfg(test)]
mod tests;
