//! 逐行 JSON 诊断输出，统一脱敏并保留首次写入错误。

use super::ProbeResult;
use serde::Serialize;
use serde_json::Value;
use std::{io::Write, sync::Mutex};

pub(super) struct Reporter {
    state: Mutex<State>,
    secret: String,
}

struct State {
    writer: Box<dyn Write + Send>,
    error: Option<String>,
}

impl Reporter {
    pub(super) fn new(writer: impl Write + Send + 'static, secret: String) -> Self {
        Self {
            state: Mutex::new(State {
                writer: Box::new(writer),
                error: None,
            }),
            secret,
        }
    }

    pub(super) fn emit(&self, kind: &str, payload: &impl Serialize) {
        let result = serde_json::to_value(payload).and_then(|mut value| {
            self.sanitize(&mut value);
            serde_json::to_vec(&serde_json::json!({"type": kind, "data": value}))
        });
        let mut state = self.state.lock().expect("诊断输出锁中毒");
        if state.error.is_some() {
            return;
        }
        let result = match result {
            Ok(mut bytes) => {
                bytes.push(b'\n');
                state
                    .writer
                    .write_all(&bytes)
                    .and_then(|_| state.writer.flush())
                    .map_err(|e| e.to_string())
            }
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = result {
            state.error = Some(error);
        }
    }

    pub(super) fn check(&self) -> ProbeResult<()> {
        match &self.state.lock().expect("诊断输出锁中毒").error {
            Some(error) => Err(format!("写入诊断输出失败: {}", self.redact(error))),
            None => Ok(()),
        }
    }

    pub(super) fn redact(&self, text: &str) -> String {
        if self.secret.is_empty() {
            text.into()
        } else {
            text.replace(&self.secret, "[已隐藏]")
        }
    }

    fn sanitize(&self, value: &mut Value) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    let normalized = key.to_ascii_lowercase().replace('-', "_");
                    if matches!(
                        normalized.as_str(),
                        "authorization"
                            | "proxy_authorization"
                            | "api_key"
                            | "x_api_key"
                            | "access_token"
                            | "cookie"
                            | "set_cookie"
                            | "encrypted_content"
                    ) && !value.is_null()
                    {
                        *value = Value::String("[已隐藏]".into());
                    } else {
                        self.sanitize(value);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.sanitize(value);
                }
            }
            Value::String(text) => *text = self.redact(text),
            _ => {}
        }
    }
}
