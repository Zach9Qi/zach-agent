//! 增量 SSE 帧解码：支持 UTF-8 跨块、CR / LF / CRLF、注释行与多行 `data`。
//!
//! 三家协议的 `event:` 行都与 `data` 内的类型字段重复，因此只提取 `data` 段；
//! 其余字段（`id`、`retry`、注释）按规范忽略。

use zach_ai_core::ModelError;

/// 单帧上限，防止异常服务端拖垮内存。
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct SseDecoder {
    /// 错误信息中的协议名称，便于定位是哪条链路出了问题。
    label: &'static str,
    line: Vec<u8>,
    data: String,
    after_cr: bool,
    first_line_done: bool,
}

impl SseDecoder {
    pub(crate) fn new(label: &'static str) -> Self {
        Self {
            label,
            line: Vec::new(),
            data: String::new(),
            after_cr: false,
            first_line_done: false,
        }
    }

    /// 喂入一段字节，返回其中完整结束的帧（每帧是拼接好的 `data` 文本）。
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ModelError> {
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
                        message: format!("{} SSE 单帧超过 8 MiB", self.label),
                        source: None,
                    });
                }
            }
        }
        Ok(frames)
    }

    fn finish_line(&mut self, frames: &mut Vec<String>) -> Result<(), ModelError> {
        let bytes = std::mem::take(&mut self.line);
        let line = std::str::from_utf8(&bytes).map_err(|err| {
            ModelError::stream_error(format!("{} SSE 包含非法 UTF-8", self.label), err)
        })?;
        // 只有流的第一行可能带 BOM。
        let line = if self.first_line_done {
            line
        } else {
            line.trim_start_matches('\u{feff}')
        };
        self.first_line_done = true;
        if line.is_empty() {
            if !self.data.is_empty() {
                // 去掉最后一个 data 行补的换行。
                self.data.pop();
                frames.push(std::mem::take(&mut self.data));
            }
            return Ok(());
        }
        // 没有冒号的行整行是字段名、值为空；`data` 单独成行是合法的空数据行。
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        if field == "data" {
            self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
            self.data.push('\n');
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! SSE 解码器对行结束符、注释、多行 data 与异常输入的处理。
    use super::*;

    #[test]
    fn decoder_supports_multiline_data_comments_and_all_line_endings() {
        let mut decoder = SseDecoder::new("测试");
        let frames = decoder
            .push(
                b"\xef\xbb\xbfevent: ping\r: heartbeat\rdata: {\"a\":\rdata: 1}\r\rdata: next\n\n",
            )
            .unwrap();
        assert_eq!(frames, vec!["{\"a\":\n1}", "next"]);
        assert!(decoder.push(b"data: partial").unwrap().is_empty());
        assert_eq!(decoder.push(b"\r\n\r\n").unwrap(), vec!["partial"]);
    }

    /// 规范允许 `data` 不带冒号（空值）以及 `data:` 后无空格，两者都要能解码。
    #[test]
    fn data_lines_without_colon_or_space_follow_the_specification() {
        let mut decoder = SseDecoder::new("测试");
        let frames = decoder.push(b"data\ndata:x\n\n").unwrap();
        assert_eq!(frames, vec!["\nx"]);
    }

    #[test]
    fn invalid_utf8_and_oversized_frames_fail_explicitly() {
        let mut decoder = SseDecoder::new("测试");
        assert!(matches!(
            decoder.push(b"data: \xff\n\n"),
            Err(ModelError::StreamError { .. })
        ));
        let mut decoder = SseDecoder::new("测试");
        assert!(matches!(
            decoder.push(&vec![b'x'; MAX_FRAME_BYTES + 1]),
            Err(ModelError::StreamError { message, .. }) if message.contains("8 MiB")
        ));
    }

    /// 多字节字符被切在两次 push 之间时不能提前按 UTF-8 校验。
    #[test]
    fn utf8_sequences_split_across_pushes_are_reassembled() {
        let mut decoder = SseDecoder::new("测试");
        let bytes = "data: 你好\n\n".as_bytes();
        let mut frames = Vec::new();
        for byte in bytes {
            frames.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(frames, vec!["你好"]);
    }
}
