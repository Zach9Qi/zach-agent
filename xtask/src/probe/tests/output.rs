//! 诊断日志脱敏及写入错误传播。

use super::super::output::Reporter;
use super::support::Buffer;
use serde_json::json;
use std::io::{self, Write};

#[test]
fn credentials_are_hidden_in_nested_values_and_error_messages() {
    let buffer = Buffer::default();
    let reporter = Reporter::new(buffer.clone(), "secret-with-quote\"".into());
    reporter.emit("sample", &json!({
        "headers": {"Authorization": "another-key", "X-Api-Key": "other", "set-cookie": "session"},
        "nested": [{"encrypted_content": "opaque", "message": "bad secret-with-quote\""}],
        "text": "正常输出",
    }));
    reporter.check().unwrap();
    let lines = buffer.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["data"]["headers"]["Authorization"], "[已隐藏]");
    assert_eq!(lines[0]["data"]["headers"]["X-Api-Key"], "[已隐藏]");
    assert_eq!(lines[0]["data"]["headers"]["set-cookie"], "[已隐藏]");
    assert_eq!(
        lines[0]["data"]["nested"][0]["encrypted_content"],
        "[已隐藏]"
    );
    assert_eq!(lines[0]["data"]["nested"][0]["message"], "bad [已隐藏]");
    assert_eq!(lines[0]["data"]["text"], "正常输出");
}

pub(super) struct BrokenWriter;

impl Write for BrokenWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "输出已关闭"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn output_failure_is_retained_instead_of_panicking_or_reporting_success() {
    let reporter = Reporter::new(BrokenWriter, "secret".into());
    reporter.emit("first", &json!({}));
    reporter.emit("second", &json!({}));
    assert!(reporter.check().unwrap_err().contains("输出已关闭"));
}
