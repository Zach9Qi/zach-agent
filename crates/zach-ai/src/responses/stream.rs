//! Responses SSE 流：协议解析器接入共用驱动，消费方丢弃流即停止拉取网络数据。

mod items;
mod parser;
mod text;

#[cfg(test)]
mod tests;

use bytes::Bytes;
use futures::Stream;
use std::time::Duration;
use zach_ai_core::{LanguageModelStream, ModelWarning, StreamPart};

use crate::transport::{sse_stream, SseParser};

pub(super) use parser::ResponsesStreamParser;

impl SseParser for ResponsesStreamParser {
    fn data(&mut self, data: &str) -> Vec<StreamPart> {
        ResponsesStreamParser::data(self, data)
    }

    fn terminal(&self) -> bool {
        self.terminal
    }

    fn failed(&self) -> bool {
        self.failed
    }

    /// Responses 的 `Finish` 只随终止事件一起发出，两者同时成立。
    fn finished(&self) -> bool {
        self.terminal
    }
}

pub(super) fn responses_stream<S, E>(
    source: S,
    parser: ResponsesStreamParser,
    warnings: Vec<ModelWarning>,
    idle: Option<Duration>,
) -> LanguageModelStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    sse_stream(super::LABEL, source, parser, warnings, idle)
}
