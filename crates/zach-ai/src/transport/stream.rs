//! 字节流 → SSE 帧 → 核心流事件的驱动循环。
//!
//! 流按消费需求拉取：消费方丢弃流即停止读取网络。三家适配器的终止语义统一为：
//! - 解析器报告 `terminal` 后不再读取源流；
//! - 源流在 `terminal` 之前结束（干净 EOF、传输错误或空闲超时）时，只有解析器既没报告过
//!   错误事件、也没发出过 `Finish`，才视为传输错误；
//!   - 已报告错误事件：服务端通常随后直接断开（FIN 或 RST 都有），再叠加一个可重试的
//!     传输错误会让上层把不可重试的厂商错误误判为传输故障并反复重试；
//!   - 已发出 `Finish`：Chat 在用量块、Anthropic 在 `message_delta` 就能收尾，之后只剩
//!     纯终止符；代理或兼容端点省掉终止符就断开时，完整响应不能被尾部错误整体判失败。

#[cfg(test)]
mod tests;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;
use zach_ai_core::{LanguageModelStream, ModelError, ModelWarning, StreamPart};

use super::sse::SseDecoder;

/// 协议解析器：把一帧 `data` 文本转换为零个或多个核心流事件。
pub(crate) trait SseParser: Send + 'static {
    /// 处理一帧 `data` 段。协议错误应以 [`StreamPart::Error`] 返回并置位 `failed`，
    /// 而不是 `Err`：解析错误不终止流，后续用量与元数据仍可收集。
    fn data(&mut self, data: &str) -> Vec<StreamPart>;

    /// 是否已收到协议层终止事件，之后不再读取网络。
    fn terminal(&self) -> bool;

    /// 是否已报告过错误事件，之后连接关闭不再视为传输故障。
    fn failed(&self) -> bool;

    /// 是否已发出 `Finish`（响应已语义完整），之后连接关闭不再视为传输故障。
    fn finished(&self) -> bool;
}

struct State<S, P> {
    source: Pin<Box<S>>,
    decoder: SseDecoder,
    parser: P,
    label: &'static str,
    /// 相邻两次收到数据之间的上限；`None` 不限制。
    idle: Option<Duration>,
    queue: VecDeque<Result<StreamPart, ModelError>>,
    ended: bool,
}

/// 从源流拉取一次的结果。
enum Pull<E> {
    Bytes(Bytes),
    Failed(E),
    /// 超过空闲上限没有收到任何数据。
    Idle(Duration),
    End,
}

/// 以 `parser` 驱动 `source`，`label` 用于错误信息中标识协议；
/// `warnings` 是请求构建阶段产生的降级警告，随首个 `StreamStart` 事件透出；
/// `idle` 是相邻两次收到数据之间的上限，超过即按传输错误结束。
pub(crate) fn sse_stream<S, E, P>(
    label: &'static str,
    source: S,
    parser: P,
    warnings: Vec<ModelWarning>,
    idle: Option<Duration>,
) -> LanguageModelStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
    P: SseParser,
{
    let state = State {
        source: Box::pin(source),
        decoder: SseDecoder::new(label),
        parser,
        label,
        idle,
        queue: VecDeque::from([Ok(StreamPart::StreamStart { warnings })]),
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
            match state.pull().await {
                Pull::Bytes(bytes) => state.feed(&bytes),
                Pull::Failed(error) => state.end(ModelError::stream_error(
                    format!("读取 {} SSE 字节流失败", state.label),
                    error,
                )),
                Pull::Idle(limit) => state.end(ModelError::StreamError {
                    message: format!(
                        "{} SSE 超过 {} 秒没有收到数据",
                        state.label,
                        limit.as_secs()
                    ),
                    source: None,
                }),
                Pull::End => state.end(ModelError::StreamError {
                    message: format!("{} SSE 在终止事件到达前结束", state.label),
                    source: None,
                }),
            }
        }
    }))
}

impl<S, E, P> State<S, P>
where
    S: Stream<Item = Result<Bytes, E>>,
    P: SseParser,
{
    async fn pull(&mut self) -> Pull<E> {
        let next = self.source.next();
        let item = match self.idle {
            Some(limit) => match tokio::time::timeout(limit, next).await {
                Ok(item) => item,
                Err(_) => return Pull::Idle(limit),
            },
            None => next.await,
        };
        match item {
            Some(Ok(bytes)) => Pull::Bytes(bytes),
            Some(Err(error)) => Pull::Failed(error),
            None => Pull::End,
        }
    }

    /// 源流在终止事件之前结束：结果尚未由事件决定时才补一个传输错误。
    fn end(&mut self, error: ModelError) {
        self.ended = true;
        if !self.settled() {
            self.queue.push_back(Err(error));
        }
    }

    /// 本轮结果是否已由事件决定（失败或完成），之后连接如何结束只是诊断信息。
    fn settled(&self) -> bool {
        self.parser.failed() || self.parser.finished()
    }

    fn feed(&mut self, bytes: &[u8]) {
        match self.decoder.push(bytes) {
            Ok(frames) => {
                for frame in frames {
                    self.queue
                        .extend(self.parser.data(&frame).into_iter().map(Ok));
                    if self.parser.terminal() {
                        self.ended = true;
                        break;
                    }
                }
            }
            Err(error) => {
                self.queue.push_back(Err(error));
                self.ended = true;
            }
        }
    }
}
