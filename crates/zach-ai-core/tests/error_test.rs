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
