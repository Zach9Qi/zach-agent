//! 模型错误的来源链与分类

use std::error::Error;
use std::io;
use zach_ai_core::ModelError;

#[test]
fn stream_error_keeps_underlying_source() {
    let io_err = io::Error::new(io::ErrorKind::ConnectionReset, "os error 10054");
    let err = ModelError::stream_error("读取响应体失败", io_err);

    assert_eq!(err.to_string(), "流式传输错误: 读取响应体失败");
    let source = err.source().expect("应保留底层错误");
    assert_eq!(source.to_string(), "os error 10054");
    assert_eq!(
        source.downcast_ref::<io::Error>().map(io::Error::kind),
        Some(io::ErrorKind::ConnectionReset)
    );
}

#[test]
fn only_transient_errors_are_retryable() {
    assert!(ModelError::RateLimit("429".to_string()).is_retryable());
    assert!(ModelError::stream_error("连接重置", "reset").is_retryable());

    assert!(!ModelError::Authentication("key 无效".to_string()).is_retryable());
    assert!(!ModelError::InvalidRequest("缺少 model".to_string()).is_retryable());
    assert!(!ModelError::provider_error("openai", "bad request", None).is_retryable());
}
