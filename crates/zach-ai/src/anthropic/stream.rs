//! Messages SSE 流：协议解析器接入共用驱动，消费方丢弃流即停止拉取网络数据。

mod blocks;
mod parser;

#[cfg(test)]
mod tests;

use bytes::Bytes;
use futures::Stream;
use zach_ai_core::{LanguageModelStream, StreamPart};

use crate::transport::{sse_stream, SseParser};

pub(super) use parser::MessagesStreamParser;

impl SseParser for MessagesStreamParser {
    fn data(&mut self, data: &str) -> Vec<StreamPart> {
        MessagesStreamParser::data(self, data)
    }

    fn terminal(&self) -> bool {
        self.terminal
    }

    fn failed(&self) -> bool {
        self.failed
    }
}

pub(super) fn messages_stream<S, E>(source: S, parser: MessagesStreamParser) -> LanguageModelStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    sse_stream(super::LABEL, source, parser)
}
