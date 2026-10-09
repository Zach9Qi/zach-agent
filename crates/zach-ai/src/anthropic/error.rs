//! Messages API HTTP 错误转换。

use reqwest::Response;
use zach_ai_core::ModelError;

pub(super) async fn http_error(response: Response) -> ModelError {
    let status = response.status();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => return ModelError::stream_error("读取 Messages 错误响应失败", error),
    };
    from_body(status.as_u16(), &body)
}

fn from_body(status: u16, body: &[u8]) -> ModelError {
    let raw = serde_json::from_slice::<serde_json::Value>(body).ok();
    let message = raw
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(serde_json::Value::as_str)
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
        // 529 是 Anthropic 的 overloaded_error：与限流一样属于可重试的暂时性故障。
        429 | 529 => ModelError::RateLimit(message),
        _ => ModelError::provider_error("anthropic", message, raw),
    }
}

#[cfg(test)]
mod tests {
    //! HTTP 错误分类与非 JSON 错误响应的回退。
    use super::*;

    #[test]
    fn authentication_rate_limit_and_overload_have_stable_categories() {
        let body = br#"{"type":"error","error":{"type":"x","message":"failure"}}"#;
        assert!(matches!(
            from_body(401, body),
            ModelError::Authentication(_)
        ));
        assert!(matches!(
            from_body(403, body),
            ModelError::Authentication(_)
        ));
        assert!(from_body(429, body).is_retryable());
        assert!(from_body(529, body).is_retryable());
        assert!(matches!(
            from_body(500, body),
            ModelError::ProviderError { raw: Some(_), .. }
        ));
        assert!(matches!(from_body(502, b"proxy failure"),
            ModelError::ProviderError { message, raw: None, .. } if message == "proxy failure"));
        assert!(from_body(400, b"").to_string().contains("HTTP 400"));
    }
}
