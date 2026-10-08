//! 将字节流逐帧解析为核心流事件；消费方丢弃流即停止拉取网络数据。

mod items;
mod parser;
mod sse;
mod text;

#[cfg(test)]
mod tests;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::collections::VecDeque;
use std::pin::Pin;
use zach_ai_core::{LanguageModelStream, ModelError, StreamPart};

pub(super) use parser::ResponsesStreamParser;
use sse::SseDecoder;

struct State<S> {
    source: Pin<Box<S>>,
    decoder: SseDecoder,
    parser: ResponsesStreamParser,
    queue: VecDeque<Result<StreamPart, ModelError>>,
    ended: bool,
}

pub(super) fn responses_stream<S, E>(
    source: S,
    parser: ResponsesStreamParser,
) -> LanguageModelStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let state = State {
        source: Box::pin(source),
        decoder: SseDecoder::default(),
        parser,
        queue: VecDeque::from([Ok(StreamPart::StreamStart { warnings: vec![] })]),
        ended: false,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(part) = state.queue.pop_front() {
                return Some((part, state));
            }
            if state.ended {
                return None;
            }
            match state.source.next().await {
                Some(Ok(bytes)) => match state.decoder.push(&bytes) {
                    Ok(frames) => {
                        for frame in frames {
                            state
                                .queue
                                .extend(state.parser.data(&frame).into_iter().map(Ok));
                            if state.parser.terminal {
                                state.ended = true;
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        state.queue.push_back(Err(error));
                        state.ended = true;
                    }
                },
                Some(Err(error)) => {
                    state.queue.push_back(Err(ModelError::stream_error(
                        "读取 Responses SSE 字节流失败",
                        error,
                    )));
                    state.ended = true;
                }
                None => {
                    state.ended = true;
                    if !state.parser.failed {
                        state.queue.push_back(Err(ModelError::StreamError {
                            message: "Responses SSE 在终止事件到达前结束".into(),
                            source: None,
                        }));
                    }
                }
            }
        }
    }))
}
