//! Chat Completions HTTP 错误转换。

use reqwest::Response;
use zach_ai_core::ModelError;

pub(super) async fn http_error(response: Response) -> ModelError {
    let status = response.status();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => return ModelError::stream_error("读取 Chat Completions 错误响应失败", error),
    };
    let raw = serde_json::from_slice::<serde_json::Value>(&body).ok();
    let message = raw
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
        })
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if body.is_empty() {
                format!("HTTP {status}")
            } else {
                String::from_utf8_lossy(&body).into_owned()
            }
        });
    match status.as_u16() {
        401 | 403 => ModelError::Authentication(message),
        429 => ModelError::RateLimit(message),
        _ => ModelError::provider_error("openai", message, raw),
    }
}
