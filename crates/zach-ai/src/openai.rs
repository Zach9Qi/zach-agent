//! OpenAI 协议族（Responses 与 Chat Completions）共用的错误方言
//!
//! 错误对象优先看 `code`，其次看 `type`：`type` 常是 `invalid_request_error` 这类大类，
//! `code` 才区分上下文超长、配额耗尽等需要不同处理的情况。
//! Chat Completions 兼容端点（DeepSeek 等）的错误码不在表中时，按 HTTP 状态码兜底。

use serde_json::Value;
use zach_ai_core::ErrorKind;

use crate::transport::ErrorDialect;

pub(crate) const DIALECT: ErrorDialect = ErrorDialect { code, kind };

fn code(error: &Value) -> Option<&str> {
    ["code", "type"]
        .into_iter()
        .find_map(|key| error.get(key)?.as_str())
}

fn kind(code: &str, _error: &Value) -> Option<ErrorKind> {
    let kind = match code {
        "invalid_api_key" => ErrorKind::Authentication,
        "context_length_exceeded" => ErrorKind::ContextLengthExceeded,
        "rate_limit_exceeded" => ErrorKind::RateLimited,
        // 状态码同为 429，但需要充值，重试无济于事
        "insufficient_quota" => ErrorKind::Rejected,
        "server_error" => ErrorKind::Unavailable,
        _ => return None,
    };
    Some(kind)
}

#[cfg(test)]
mod tests {
    //! OpenAI 错误码的读取顺序与到类别的映射。
    use super::*;
    use serde_json::json;

    #[test]
    fn code_is_preferred_over_the_broad_type() {
        let error = json!({"type": "invalid_request_error", "code": "context_length_exceeded"});
        assert_eq!(code(&error), Some("context_length_exceeded"));
        let error = json!({"type": "invalid_request_error", "code": null});
        assert_eq!(code(&error), Some("invalid_request_error"));
        // 部分兼容端点的 code 是数字，退回 type
        let error = json!({"type": "server_error", "code": 500});
        assert_eq!(code(&error), Some("server_error"));
    }

    #[test]
    fn quota_exhaustion_is_not_retried_although_it_shares_status_429_with_rate_limits() {
        let classify = |code: &str| DIALECT.error("openai", &json!({"code": code})).kind();
        assert_eq!(classify("rate_limit_exceeded"), ErrorKind::RateLimited);
        assert_eq!(classify("insufficient_quota"), ErrorKind::Rejected);
        assert_eq!(
            classify("context_length_exceeded"),
            ErrorKind::ContextLengthExceeded
        );
        assert_eq!(classify("invalid_api_key"), ErrorKind::Authentication);
        assert_eq!(classify("server_error"), ErrorKind::Unavailable);
        assert_eq!(kind("invalid_request_error", &json!({})), None);
    }
}
