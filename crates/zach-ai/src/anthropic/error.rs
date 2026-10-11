//! Messages 错误方言：错误码是错误对象的 `type`
//!
//! HTTP 错误正文与流内 `error` 事件的错误对象形状相同，都经由这里分类。

use serde_json::Value;
use zach_ai_core::ErrorKind;

use crate::transport::ErrorDialect;

pub(super) const DIALECT: ErrorDialect = ErrorDialect { code, kind };

fn code(error: &Value) -> Option<&str> {
    error.get("type")?.as_str()
}

/// 未列出的错误码交给 HTTP 状态码兜底（流内事件则归为不可重试的拒绝）
fn kind(code: &str, error: &Value) -> Option<ErrorKind> {
    let kind = match code {
        "authentication_error" | "permission_error" => ErrorKind::Authentication,
        "rate_limit_error" => ErrorKind::RateLimited,
        "overloaded_error" | "api_error" => ErrorKind::Unavailable,
        // 上下文超长没有独立的错误码，只能识别描述
        "invalid_request_error" if prompt_too_long(error) => ErrorKind::ContextLengthExceeded,
        "invalid_request_error" | "not_found_error" | "request_too_large" => ErrorKind::Rejected,
        _ => return None,
    };
    Some(kind)
}

fn prompt_too_long(error: &Value) -> bool {
    error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.starts_with("prompt is too long"))
}

#[cfg(test)]
mod tests {
    //! Messages 错误码到类别的映射。
    use super::*;
    use serde_json::json;

    fn classify(kind: &str, message: &str) -> ErrorKind {
        DIALECT
            .error("anthropic", &json!({"type": kind, "message": message}))
            .kind()
    }

    #[test]
    fn documented_error_types_map_to_kinds_by_how_they_are_handled() {
        assert_eq!(
            classify("overloaded_error", "Overloaded"),
            ErrorKind::Unavailable
        );
        assert_eq!(classify("api_error", "boom"), ErrorKind::Unavailable);
        assert_eq!(classify("rate_limit_error", "slow"), ErrorKind::RateLimited);
        assert_eq!(
            classify("permission_error", "no"),
            ErrorKind::Authentication
        );
        assert_eq!(classify("not_found_error", "no model"), ErrorKind::Rejected);
        assert_eq!(
            classify(
                "invalid_request_error",
                "prompt is too long: 208310 tokens > 200000 maximum"
            ),
            ErrorKind::ContextLengthExceeded
        );
        assert_eq!(
            classify("invalid_request_error", "bad"),
            ErrorKind::Rejected
        );
    }

    #[test]
    fn unknown_error_types_are_left_to_the_status_fallback() {
        assert_eq!(kind("future_error", &json!({})), None);
    }
}
