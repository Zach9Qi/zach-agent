//! 增量 SSE 帧解码，支持 UTF-8 跨块、CR/LF、注释与多行 data。
//!
//! Anthropic 的 `event:` 行与 `data.type` 重复，这里只提取 data 段。

use zach_ai_core::ModelError;

const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

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
                self.finish_line(&mut frames)?;
                self.after_cr = byte == b'\r';
            } else {
                self.line.push(byte);
                if self.line.len() + self.data.len() > MAX_FRAME_BYTES {
                    return Err(ModelError::StreamError {
                        message: "Messages SSE 单帧超过 8 MiB".into(),
                        source: None,
                    });
                }
            }
        }
        Ok(frames)
    }

    fn finish_line(&mut self, frames: &mut Vec<String>) -> Result<(), ModelError> {
        let bytes = std::mem::take(&mut self.line);
        let line = std::str::from_utf8(&bytes)
            .map_err(|err| ModelError::stream_error("Messages SSE 包含非法 UTF-8", err))?;
        let line = if self.first_line_done {
            line
        } else {
            line.trim_start_matches('\u{feff}')
        };
        self.first_line_done = true;
        if line.is_empty() {
            if !self.data.is_empty() {
                self.data.pop();
                frames.push(std::mem::take(&mut self.data));
            }
        } else {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            if field == "data" {
                self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
                self.data.push('\n');
            }
        }
        Ok(())
    }
}
