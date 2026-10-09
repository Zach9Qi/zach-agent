//! Chat Completions SSE 帧解码。

use zach_ai_core::ModelError;

#[derive(Default)]
pub(super) struct SseDecoder {
    line: Vec<u8>,
    data: String,
    after_cr: bool,
    first_line_done: bool,
}

impl SseDecoder {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ModelError> {
        let mut frames = Vec::new();
        for &byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.line(&mut frames)?;
                self.after_cr = byte == b'\r';
            } else {
                self.line.push(byte);
                if self.line.len() + self.data.len() > 8 * 1024 * 1024 {
                    return Err(ModelError::StreamError {
                        message: "Chat Completions SSE 单帧超过 8 MiB".into(),
                        source: None,
                    });
                }
            }
        }
        Ok(frames)
    }
    fn line(&mut self, frames: &mut Vec<String>) -> Result<(), ModelError> {
        let bytes = std::mem::take(&mut self.line);
        let value = std::str::from_utf8(&bytes).map_err(|error| {
            ModelError::stream_error("Chat Completions SSE 包含非法 UTF-8", error)
        })?;
        let value = if self.first_line_done {
            value
        } else {
            value.trim_start_matches('\u{feff}')
        };
        self.first_line_done = true;
        if value.is_empty() {
            if !self.data.is_empty() {
                self.data.pop();
                frames.push(std::mem::take(&mut self.data));
            }
        } else if value.starts_with("data:") {
            self.data.push_str(
                value
                    .strip_prefix("data:")
                    .unwrap()
                    .strip_prefix(' ')
                    .unwrap_or(value.strip_prefix("data:").unwrap()),
            );
            self.data.push('\n');
        }
        Ok(())
    }
}
