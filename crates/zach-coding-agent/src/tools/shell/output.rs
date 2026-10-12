//! 命令输出的有界缓冲：保留末尾供模型查看，按需保留开头供落盘

/// 落盘时最多保留的输出字节数，超出部分只计数不保存
pub(super) const SPILL_CAP: usize = 8 * 1024 * 1024;

/// 命令输出缓冲
pub(super) struct OutputBuffer {
    /// 从头开始的输出，最多 `head_cap` 字节
    head: Vec<u8>,
    head_cap: usize,
    /// 末尾输出，摊还裁剪，长度在 `tail_cap` 与其两倍之间
    tail: Vec<u8>,
    tail_cap: usize,
    total_bytes: usize,
    newlines: usize,
    ends_with_newline: bool,
}

impl OutputBuffer {
    /// `tail_cap` 为保留给模型的末尾字节数，`head_cap` 为保留给落盘的开头字节数（0 表示不落盘）
    pub(super) fn new(tail_cap: usize, head_cap: usize) -> Self {
        Self {
            head: Vec::new(),
            head_cap,
            tail: Vec::new(),
            tail_cap,
            total_bytes: 0,
            newlines: 0,
            ends_with_newline: false,
        }
    }

    pub(super) fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let room = self.head_cap.saturating_sub(self.head.len());
        self.head.extend_from_slice(&chunk[..room.min(chunk.len())]);
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > self.tail_cap * 2 {
            let excess = self.tail.len() - self.tail_cap;
            self.tail.drain(..excess);
        }
        self.total_bytes += chunk.len();
        self.newlines += chunk.iter().filter(|&&byte| byte == b'\n').count();
        self.ends_with_newline = chunk.ends_with(b"\n");
    }

    /// 输出总字节数
    pub(super) fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// 输出总行数（末尾不带换行的残行也算一行）
    pub(super) fn total_lines(&self) -> usize {
        let partial = self.total_bytes > 0 && !self.ends_with_newline;
        self.newlines + usize::from(partial)
    }

    /// 是否有输出没能留在末尾缓冲中
    pub(super) fn dropped(&self) -> bool {
        self.total_bytes > self.tail_bytes().len()
    }

    /// 末尾输出的文本；有内容被丢弃时从第一个完整行开始，避免半截行与半个字符
    pub(super) fn tail_text(&self) -> String {
        let mut bytes = self.tail_bytes();
        if self.dropped() {
            if let Some(newline) = bytes.iter().position(|&byte| byte == b'\n') {
                bytes = &bytes[newline + 1..];
            } else {
                let skip = bytes.iter().take(3).take_while(|&&b| b & 0xc0 == 0x80);
                bytes = &bytes[skip.count()..];
            }
        }
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// 落盘用的开头输出，以及它是否就是完整输出
    pub(super) fn head(&self) -> (&[u8], bool) {
        (&self.head, self.head.len() == self.total_bytes)
    }

    fn tail_bytes(&self) -> &[u8] {
        &self.tail[self.tail.len().saturating_sub(self.tail_cap)..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_outputs_are_kept_whole() {
        let mut buffer = OutputBuffer::new(100, 100);
        buffer.push(b"a\nb");
        buffer.push(b"\nc");
        assert_eq!(buffer.tail_text(), "a\nb\nc");
        assert_eq!((buffer.total_lines(), buffer.total_bytes()), (3, 5));
        assert!(!buffer.dropped());
        assert_eq!(buffer.head(), (&b"a\nb\nc"[..], true));
    }

    /// 末尾缓冲裁掉开头后，从第一个完整行开始展示
    #[test]
    fn tail_keeps_recent_output_from_a_line_boundary() {
        let mut buffer = OutputBuffer::new(8, 0);
        for i in 0..10 {
            buffer.push(format!("line{i}\n").as_bytes());
        }
        assert!(buffer.dropped());
        assert_eq!(buffer.tail_text(), "line9\n");
        assert_eq!(buffer.total_lines(), 10);
        assert_eq!(buffer.head(), (&b""[..], false));
    }

    #[test]
    fn tail_without_newlines_starts_on_a_char_boundary() {
        let mut buffer = OutputBuffer::new(4, 0);
        buffer.push("一二三".as_bytes());
        assert_eq!(buffer.tail_text(), "三");
    }

    #[test]
    fn head_stops_at_its_cap() {
        let mut buffer = OutputBuffer::new(4, 3);
        buffer.push(b"abcdef");
        assert_eq!(buffer.head(), (&b"abc"[..], false));
    }
}
